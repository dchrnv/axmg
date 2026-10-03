use serde_json::json;
use shelf::death::{DeathLog, FocusSet};
use shelf::merge::TieBreak;
use shelf::store::Store;
use shelf::wheel::WheelState;
use std::sync::{Arc, RwLock};
use tracing::info;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TimelineEntry {
    pub revolution: u32,
    pub timestamp: u64,
    pub context_tag: String,
    pub text: String,
    pub surprise_score: f64,
    pub verdict: String,
    pub new_tokens: usize,
}

pub struct KernelAdapter {
    wheel: tokio::sync::RwLock<WheelState>,
    death_log: tokio::sync::RwLock<DeathLog>,
    focus_snapshot: RwLock<Arc<FocusSet>>,
    timeline: tokio::sync::RwLock<Vec<TimelineEntry>>,
    db_path: std::path::PathBuf,
    tl_path: std::path::PathBuf,
}

impl Default for KernelAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelAdapter {
    pub fn new() -> Self {
        let db_path = std::env::var("AXMG_DB_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("axmg_store.redb"));

        let tl_path = std::env::var("AXMG_TIMELINE_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("axmg_timeline.jsonl"));

        Self::with_paths(db_path, tl_path)
    }

    pub fn with_paths(db_path: std::path::PathBuf, tl_path: std::path::PathBuf) -> Self {
        let (store, mut death_log) = match shelf::persist::load_all(&db_path) {
            Ok((s, dl)) => {
                tracing::info!(
                    "Loaded store from {}, tokens: {}, focus events: {}",
                    db_path.display(),
                    s.len(),
                    dl.events().len()
                );
                (s, dl)
            }
            Err(e) => {
                tracing::info!("Starting with fresh store ({}): creating factory", e);
                let mut s = Store::new();
                s.init_factory();
                let dl = DeathLog::new();
                (s, dl)
            }
        };

        // Загрузка сохраненного таймлайна событий
        let mut timeline = Vec::new();
        if tl_path.exists() {
            if let Ok(file) = std::fs::File::open(&tl_path) {
                use std::io::BufRead;
                let reader = std::io::BufReader::new(file);
                for line in reader.lines().flatten() {
                    if let Ok(entry) = serde_json::from_str::<TimelineEntry>(&line) {
                        timeline.push(entry);
                    }
                }
            }
            tracing::info!("Loaded {} timeline events from {}", timeline.len(), tl_path.display());
        }

        let mut wheel = WheelState::with_store(store);
        for entry in &timeline {
            let ids = shelf::recognizer::StreamingRecognizer::recognize(&wheel.store, entry.text.as_bytes());
            wheel.sequence.ids.extend(ids);
        }
        if let Some(last) = timeline.last() {
            wheel.revolution = last.revolution;
        }

        if death_log.events().is_empty() {
            death_log.mark(&wheel);
        }
        let initial_snapshot = death_log.snapshot(&wheel, 20);

        Self {
            wheel: tokio::sync::RwLock::new(wheel),
            death_log: tokio::sync::RwLock::new(death_log),
            focus_snapshot: RwLock::new(Arc::new(initial_snapshot)),
            timeline: tokio::sync::RwLock::new(timeline),
            db_path,
            tl_path,
        }
    }

    pub async fn remember_bytes(&self, bytes: &[u8], context_tag: Option<&str>) -> serde_json::Value {
        let mut wheel = self.wheel.write().await;
        let mut death_log = self.death_log.write().await;

        // 1. Вращаем колесо: атомарно вычисляет S-static на замороженном сторе,
        // выполняет волну слияний и рассчитывает S-growth по реально рождённым токенам БЕЗ клонирования стора.
        let report = wheel.turn(bytes, 2, TieBreak::BirthOrder);

        // 2. Обновляем активный фокус (вытеснение K=3 и срез по весу)
        let mark_outcome = death_log.mark(&wheel);
        let cut_outcome = death_log.weight_cut(&wheel);

        // 3. Формируем новый снимок активного фокуса и атомарно публикуем для читателей (lock-free)
        let new_focus = death_log.snapshot(&wheel, 20);
        *self.focus_snapshot.write().unwrap() = Arc::new(new_focus);

        // 4. Надежный транзакционный сброс в redb (обе таблицы Store + DeathLog в единой транзакции)
        if let Err(e) = shelf::persist::save_all(&wheel.store, &death_log, &self.db_path) {
            tracing::error!("Failed to save store and death log to disk: {}", e);
        }

        let is_novel = report.s_growth > 0.0;
        let verdict_str = match report.verdict {
            shelf::SurpriseVerdict::Familiar => "FAMILIAR",
            shelf::SurpriseVerdict::NovelGrowth => "NOVEL_GROWTH",
            shelf::SurpriseVerdict::Noise => "NOISE",
        };

        // 5. Запись в таймлайн событий
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let tag = context_tag.unwrap_or("general").to_string();
        let display_text = match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => format!("<binary data: {} bytes>", bytes.len()),
        };

        let entry = TimelineEntry {
            revolution: report.revolution,
            timestamp,
            context_tag: tag.clone(),
            text: display_text,
            surprise_score: (report.surprise_score * 1000.0).round() / 1000.0,
            verdict: verdict_str.to_string(),
            new_tokens: report.new_tokens_count,
        };

        {
            let mut tl = self.timeline.write().await;
            tl.push(entry.clone());
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&self.tl_path) {
                if let Ok(json_line) = serde_json::to_string(&entry) {
                    let _ = writeln!(file, "{}", json_line);
                }
            }
        }

        info!(
            "Remembered {} bytes [tag: {}]. Rev: {}, Score: {:.4}, Focus: {} (+{}/-{}, cut: {}), Verdict: {:?}",
            bytes.len(),
            tag,
            report.revolution,
            report.surprise_score,
            death_log.focused_set().len(),
            mark_outcome.newly_focused,
            mark_outcome.newly_unfocused,
            cut_outcome.cut_count,
            report.verdict
        );

        json!({
            "status": "STORED",
            "revolution": report.revolution,
            "bytes_count": bytes.len(),
            "context_tag": tag,
            "surprise_static": report.s_static,
            "surprise_growth": report.s_growth,
            "surprise_score": report.surprise_score,
            "verdict": verdict_str,
            "new_tokens": report.new_tokens_count,
            "active_focus_count": death_log.focused_set().len(),
            "newly_focused": mark_outcome.newly_focused,
            "newly_unfocused": mark_outcome.newly_unfocused,
            "weight_cut_count": cut_outcome.cut_count,
            "is_novel_event": is_novel
        })
    }

    pub async fn remember(&self, text: &str, context_tag: Option<&str>) -> serde_json::Value {
        self.remember_bytes(text.as_bytes(), context_tag).await
    }

    pub async fn recall(&self, query: &str, limit: usize, frequency_floor: u64, ppmi_threshold: f64) -> serde_json::Value {
        let focus = self.focus_snapshot.read().unwrap().clone();
        let wheel = self.wheel.read().await;
        let config = shelf::RecallConfig {
            limit,
            frequency_floor,
            ppmi_threshold,
            min_level: 1,
            window_min: 2,
            window_max: 5,
        };

        let focus_ref = if focus.is_empty() { None } else { Some(focus.tokens()) };
        let result = shelf::recall(&wheel.store, &wheel.sequence.ids, query.as_bytes(), focus_ref, &config);
        let context_string = result.to_context_string();

        let associations: Vec<_> = result
            .candidates
            .into_iter()
            .map(|cand| {
                json!({
                    "token_id": cand.token,
                    "associated_text": cand.text,
                    "relevance_score": cand.score,
                    "ppmi": cand.ppmi,
                    "raw_weight": cand.raw_weight,
                    "level": cand.level
                })
            })
            .collect();

        json!({
            "query": query,
            "parsed_tokens": result.query_tokens.len(),
            "associations": associations,
            "context_string": context_string
        })
    }

    pub async fn check_surprise_bytes(&self, candidate_bytes: &[u8]) -> serde_json::Value {
        let wheel = self.wheel.read().await;
        let report = shelf::surprise(&wheel.store, candidate_bytes);

        let verdict_str = match report.verdict {
            shelf::SurpriseVerdict::Familiar => "FAMILIAR",
            shelf::SurpriseVerdict::NovelGrowth => "NOVEL_GROWTH",
            shelf::SurpriseVerdict::Noise => "NOISE",
        };

        json!({
            "bytes_count": candidate_bytes.len(),
            "surprise_static": report.s_static,
            "surprise_growth": report.s_growth,
            "surprise_score": report.surprise_score,
            "verdict": verdict_str
        })
    }

    pub async fn check_surprise(&self, candidate_text: &str) -> serde_json::Value {
        self.check_surprise_bytes(candidate_text.as_bytes()).await
    }

    pub async fn read_focused(&self) -> serde_json::Value {
        // Неблокирующий lock-free доступ: клонирование Arc-снапшота за наносекунды без блокировки колеса
        let focus = self.focus_snapshot.read().unwrap().clone();
        let top_concepts: Vec<_> = focus
            .top_concepts()
            .iter()
            .map(|c| {
                json!({
                    "token_id": c.token_id,
                    "text": c.text,
                    "level": c.level,
                    "ppmi_weight": c.ppmi_weight
                })
            })
            .collect();

        json!({
            "revolution": focus.revolution(),
            "focus_count": focus.len(),
            "top_concepts": top_concepts
        })
    }

    pub async fn read_stats(&self) -> serde_json::Value {
        let focus = self.focus_snapshot.read().unwrap().clone();
        let wheel = self.wheel.read().await;

        let total_tokens = wheel.store.len();
        let focus_tokens = focus.len();
        let active_focus_ratio = if total_tokens > 0 {
            focus_tokens as f64 / total_tokens as f64
        } else {
            0.0
        };

        let seq_len = wheel.sequence.ids.len();
        let (raw_bytes, compression_ratio) = if seq_len > 0 && total_tokens > 1 {
            let total_raw: usize = wheel.sequence.ids.iter().map(|&id| wheel.store.bytes_of(id).len()).sum();
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

        json!({
            "total_tokens": total_tokens,
            "current_revolution": wheel.revolution,
            "sequence_length": seq_len,
            "raw_stream_bytes": raw_bytes,
            "compression_ratio": compression_ratio,
            "active_focus_tokens": focus_tokens,
            "active_focus_ratio": (active_focus_ratio * 10000.0).round() / 10000.0
        })
    }

    pub async fn read_timeline(&self) -> serde_json::Value {
        let tl = self.timeline.read().await;
        let wheel = self.wheel.read().await;

        // Возвращаем до 50 последних событий в обратном хронологическом порядке (сначала свежие)
        let recent: Vec<_> = tl.iter().rev().take(50).cloned().collect();

        json!({
            "current_revolution": wheel.revolution,
            "total_events": tl.len(),
            "timeline": recent
        })
    }
}
