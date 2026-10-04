//! # recall — Ассоциативное вспоминание по Merkle DAG и PPMI-графу
//!
//! Реализует субграфный поиск и извлечение релевантных концептов вокруг произвольного запроса:
//! 1. Свёртка `query: &[u8]` через `StreamingRecognizer` в последовательность токенов `query_tokens`.
//! 2. Поиск ассоциированных токенов через матрицу PPMI:
//!    `Neighbors(t) = { v | PPMI(t, v) > threshold && v in FocusSet }`
//! 3. Ранжирование кандидатов по совместному весу:
//!    `Score(v) = sum_{t in query_tokens} PPMI(t, v) * Weight(v)`
//! 4. Рекурсивное декодирование `bytes_of(v)` и форматирование в компактный контекст.

use crate::context::{filter_by_min_level, ContextExtractor, Window};
use crate::cooc::{CooAccumulator, Csr};
use crate::ppmi;
use crate::recognizer::StreamingRecognizer;
use crate::store::{Store, TokenId};
use std::collections::{HashMap, HashSet};

/// Проверяет, является ли `descendant` потомком `ancestor` в графе композиции Store.
pub fn is_descendant(store: &Store, ancestor: TokenId, descendant: TokenId) -> bool {
    if ancestor == descendant {
        return true;
    }
    let (level, children) = store.get(ancestor);
    if level <= store.level_of(descendant) {
        return false;
    }
    for &child in children {
        if is_descendant(store, child, descendant) {
            return true;
        }
    }
    false
}

/// Конфигурация ассоциативного вспоминания (`recall`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecallConfig {
    /// Максимальное число возвращаемых ассоциаций. Default: 5
    pub limit: usize,
    /// Минимальная частота совместной встречаемости в окне. Default: 1
    pub frequency_floor: u64,
    /// Минимальный порог PPMI в битах. Default: 0.1
    pub ppmi_threshold: f64,
    /// Минимальный уровень композиции токенов для отсечения посимвольного шума. Default: 1
    pub min_level: u32,
    /// Минимальная дистанция окна контекста (Window). Default: 1
    pub window_min: usize,
    /// Максимальная дистанция окна контекста (Window). Default: 5
    pub window_max: usize,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            limit: 5,
            frequency_floor: 1,
            ppmi_threshold: 0.1,
            min_level: 1,
            window_min: 1,
            window_max: 5,
        }
    }
}

/// Ассоциированный концепт/воспоминание.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecallCandidate {
    pub token: TokenId,
    pub bytes: Vec<u8>,
    pub text: String,
    pub score: f64,
    pub ppmi: f64,
    pub raw_weight: u64,
    pub level: u32,
}

/// Результат выполнения операции `recall`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecallResult {
    pub query_tokens: Vec<TokenId>,
    pub candidates: Vec<RecallCandidate>,
}

impl RecallResult {
    /// Компактное представление ассоциаций в виде строки для передачи в системный промпт LLM.
    pub fn to_context_string(&self) -> String {
        let texts: Vec<&str> = self
            .candidates
            .iter()
            .map(|c| c.text.trim())
            .filter(|s| !s.is_empty())
            .collect();
        texts.join(", ")
    }
}

/// Поиск ассоциаций по уже построенной матрице со-встречаемости CSR.
pub fn recall_from_csr(
    store: &Store,
    csr: &Csr,
    query_tokens: &[TokenId],
    focused: Option<&HashSet<TokenId>>,
    config: &RecallConfig,
) -> Vec<RecallCandidate> {
    if query_tokens.is_empty() {
        return Vec::new();
    }

    let valid_links = ppmi::valid_links(csr, config.frequency_floor, config.ppmi_threshold);

    // candidate -> (sum_ppmi, max_ppmi, total_raw_weight)
    let mut candidate_stats: HashMap<TokenId, (f64, f64, u64)> = HashMap::new();

    // Расширяем поиск прямыми детьми составных токенов запроса, чтобы улавливать ассоциации к частям фразы
    let mut expanded_query = HashSet::new();
    for &q in query_tokens {
        expanded_query.insert(q);
        let (level, children) = store.get(q);
        if level > 0 {
            for &c in children {
                expanded_query.insert(c);
            }
        }
    }

    for &q in &expanded_query {
        for entry in &valid_links {
            let neighbor = if entry.token == q {
                Some(entry.context)
            } else if entry.context == q {
                Some(entry.token)
            } else {
                None
            };

            if let Some(cand) = neighbor {
                // Не возвращаем в качестве ассоциации сам токен запроса
                if expanded_query.contains(&cand) {
                    continue;
                }

                // Проверка фильтра активного фокуса
                if let Some(focus_set) = focused {
                    if !focus_set.contains(&cand) {
                        continue;
                    }
                }

                // Проверка минимального уровня композиции
                if store.level_of(cand) < config.min_level {
                    continue;
                }

                let stat = candidate_stats.entry(cand).or_insert((0.0, 0.0, 0));
                stat.0 += entry.ppmi;
                stat.1 = stat.1.max(entry.ppmi);
                stat.2 += entry.raw_weight;
            }
        }
    }

    let mut ranked: Vec<RecallCandidate> = candidate_stats
        .into_iter()
        .map(|(token, (sum_ppmi, max_ppmi, raw_weight))| {
            let level = store.level_of(token);
            let count = store.count_of(token).max(1);

            // Score(v) = sum_{t} PPMI(t, v) * Weight(v)
            let weight = (1.0 + (count as f64).log2()) * (1.0 + 0.5 * (level as f64));
            let score = sum_ppmi * weight;

            let bytes = store.bytes_of(token);
            let text = String::from_utf8_lossy(&bytes).to_string();

            RecallCandidate {
                token,
                bytes,
                text,
                score,
                ppmi: max_ppmi,
                raw_weight,
                level,
            }
        })
        .collect();

    // Сортировка по убыванию совместного скора
    ranked.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Дедупликация одинаковых текстовых представлений
    let mut unique_texts = HashSet::new();
    ranked.retain(|c| {
        let trimmed = c.text.trim();
        if trimmed.len() < 2 {
            return false;
        }
        unique_texts.insert(trimmed.to_string())
    });

    ranked.truncate(config.limit);
    ranked
}

/// Полный субграфный цикл ассоциативного вспоминания (`recall`):
/// `Запрос (байты) -> StreamingRecognizer -> PPMI-граф по sequence -> Ранжирование -> Декодирование`
pub fn recall(
    store: &Store,
    sequence: &[TokenId],
    query_bytes: &[u8],
    focused: Option<&HashSet<TokenId>>,
    config: &RecallConfig,
) -> RecallResult {
    let query_tokens = StreamingRecognizer::recognize(store, query_bytes);
    if query_tokens.is_empty() || sequence.is_empty() {
        return RecallResult {
            query_tokens,
            candidates: Vec::new(),
        };
    }

    let window = Window::range(config.window_min, config.window_max);
    let events = window.extract(store, sequence);

    // Применяем фильтр по min_level. При нехватке составных событий — мягкий fallback к min_level = 0.
    let mut filtered = filter_by_min_level(store, events.clone(), config.min_level);
    let effective_config = if filtered.is_empty() && config.min_level > 0 {
        filtered = filter_by_min_level(store, events, 0);
        let mut c = config.clone();
        c.min_level = 0;
        c
    } else {
        config.clone()
    };

    let mut coo = CooAccumulator::new();
    coo.accumulate(&filtered);
    let csr = coo.bake_to_csr();

    // Также находим токены в sequence, которые содержат query_tokens как потомков (или наоборот)
    let mut target_query_tokens = query_tokens.clone();
    for &s in sequence {
        for &q in &query_tokens {
            if is_descendant(store, s, q) || is_descendant(store, q, s) {
                target_query_tokens.push(s);
            }
        }
    }
    target_query_tokens.sort_unstable();
    target_query_tokens.dedup();

    let mut candidates = recall_from_csr(store, &csr, &target_query_tokens, focused, &effective_config);

    // Если среди кандидатов пусто, но есть составные токены последовательности, содержащие запрос:
    if candidates.is_empty() {
        for &s in sequence {
            for &q in &query_tokens {
                if (is_descendant(store, s, q) || is_descendant(store, q, s)) && s != q {
                    let level = store.level_of(s);
                    if level >= effective_config.min_level {
                        let bytes = store.bytes_of(s);
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        candidates.push(RecallCandidate {
                            token: s,
                            bytes,
                            text,
                            score: 1.0,
                            ppmi: 1.0,
                            raw_weight: store.count_of(s),
                            level,
                        });
                    }
                }
            }
        }
        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        let mut unique_texts = HashSet::new();
        candidates.retain(|c| {
            let trimmed = c.text.trim();
            trimmed.len() >= 2 && unique_texts.insert(trimmed.to_string())
        });
        candidates.truncate(effective_config.limit);
    }

    RecallResult {
        query_tokens,
        candidates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::TieBreak;
    use crate::wheel::WheelState;

    #[test]
    fn test_recall_associates_related_words() {
        let mut wheel = WheelState::new();
        let corpus = b"the cat sat on the mat. the dog sat on the rug. the cat chased the dog. the dog barked at the cat.";
        wheel.turn(corpus, 2, TieBreak::BirthOrder);

        let config = RecallConfig::default();
        let result = recall(&wheel.store, &wheel.sequence.ids, b"cat", None, &config);
        println!("Candidates for 'cat': {:?}", result.candidates);
        assert!(!result.candidates.is_empty(), "recall должен найти ассоциации к 'cat'");
        let context_str = result.to_context_string();
        assert!(!context_str.is_empty());
    }

    #[test]
    fn test_recall_empty_query_returns_empty() {
        let wheel = WheelState::new();
        let config = RecallConfig::default();
        let result = recall(&wheel.store, &wheel.sequence.ids, b"", None, &config);
        assert!(result.candidates.is_empty());
        assert_eq!(result.to_context_string(), "");
    }
}
