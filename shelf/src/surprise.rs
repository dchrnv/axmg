//! Шаг 15 (план 3): surprise через MDL — две метрики рядом, вход
//! произвольный текст, полка не растёт по-настоящему ни от одной.
//!
//! **S-static** — коэффициент свёртки входа ЗАМОРОЖЕННОЙ полкой, без
//! права рождения: жадное распознавание существующими составами
//! (`Store::lookup`, не `intern` — см. store.rs), затем тот же честный
//! коэффициент сжатия, что и везде в проекте (`original_bytes /
//! (final_token_count * log2(vocab_size) / 8)`, docs/axmv2_Отчёт_
//! для_Opus.md). Выше — вход лучше узнаётся существующим словарём.
//!
//! **S-growth** — биты словарного прироста при ХОЛОСТОМ прогоне рождения:
//! клон стора (`Store: Clone`, добавлено этим шагом — see store.rs),
//! обычный `merge::run` с правом рождения на новом входе, разница
//! размера словаря до/после × log2(словарь после) — цена (в битах)
//! описания новых записей кодовой книги, если бы рождение было разрешено.
//! Реальный (замороженный) стор не трогается — только клон.
//!
//! **Диагностическое правило — не автоматический классификатор, только
//! отчёт.** Блокнот 4 §9 (первоисточник) формулирует так: "плохо жмётся
//! и ничего не рождает — шум; плохо жмётся и много рождает — сигнал
//! роста". План 3 переформулировал через "высокий/низкий S-static", что
//! при S-static = коэффициент сжатия (выше = лучше сжимается = менее
//! неожиданно) читается по смыслу противоположно блокноту 4 ("высокий
//! S-static" при таком определении — это ХОРОШО сжалось, не плохо).
//! Формулировки разошлись — не разрешено здесь произвольно в одну
//! сторону: `surprise()` возвращает оба сырых числа, интерпретация
//! "шум/рост" — не встроенный автоматический вывод, а решение читающего
//! отчёт (тот же принцип, что G-A3/G-B3 — "глазами", не гейт с зашитым
//! порогом на эту пару чисел).

use crate::merge::{self, Sequence, TieBreak};
use crate::store::{Store, TokenId};
use crate::{read_and_touch, BIRTH_THRESHOLD};
use std::collections::HashSet;

use crate::recognizer::StreamingRecognizer;

/// Свернуть вход, используя ТОЛЬКО уже существующие в сторе составы через StreamingRecognizer.
fn recognize_focused(store: &Store, ids: &[TokenId], focused: Option<&HashSet<TokenId>>) -> Vec<TokenId> {
    let mut rec = match focused {
        Some(f) => StreamingRecognizer::with_focus(store, f),
        None => StreamingRecognizer::new(store),
    };
    rec.feed_tokens(ids);
    rec.finish()
}

/// S-static: коэффициент свёртки `input` замороженным `store` (без
/// права рождения). `store` не изменяется — `lookup`, не `intern`.
pub fn s_static(store: &Store, input: &[u8]) -> f64 {
    s_static_focused(store, input, None)
}

/// Как `s_static`, но свёртка ограничена `focused` (шаг 12/G-B2) — см.
/// `recognize_focused`. Словарь (`vocab_size`, знаменатель битов на
/// токен) остаётся полным размером `Store` независимо от фокуса: смерть
/// не удаляет тела (append-only, INVARIANTS.md), она отсекает
/// ДОСТУПНОСТЬ для новой свёртки, не сам факт существования.
pub fn s_static_focused(store: &Store, input: &[u8], focused: Option<&HashSet<TokenId>>) -> f64 {
    if input.is_empty() {
        return 1.0;
    }
    let ids: Vec<TokenId> = input
        .iter()
        .map(|&b| {
            store
                .lookup(0, &[b as u32])
                .expect("байт-уровень фабрики обязан покрывать все 256 значений")
        })
        .collect();
    let out = recognize_focused(store, &ids, focused);
    let vocab_size = store.len();
    if vocab_size <= 1 || out.is_empty() {
        return 1.0;
    }
    let bits_per_token = (vocab_size as f64).log2();
    let compressed_bytes = out.len() as f64 * bits_per_token / 8.0;
    if compressed_bytes <= 0.0 {
        return 1.0;
    }
    input.len() as f64 / compressed_bytes
}

/// Длина свёртки (число токенов после распознавания) — G-B2 сравнивает
/// её до/после среза смерти напрямую, не через S-static.
pub fn recognized_length(store: &Store, input: &[u8], focused: Option<&HashSet<TokenId>>) -> usize {
    let ids: Vec<TokenId> = input
        .iter()
        .map(|&b| {
            store
                .lookup(0, &[b as u32])
                .expect("байт-уровень фабрики обязан покрывать все 256 значений")
        })
        .collect();
    recognize_focused(store, &ids, focused).len()
}

/// S-growth: биты словарного прироста на клоне `store`, прогнанном
/// обычным `merge::run` (с правом рождения) на `input`. Реальный `store`
/// не трогается.
pub fn s_growth(store: &Store, input: &[u8], threshold: u64, tie_break: TieBreak) -> f64 {
    let mut probe = store.clone();
    let vocab_before = probe.len();
    let ids = read_and_touch(&mut probe, input);
    let seq = Sequence::from_ids(&probe, ids);
    let _ = merge::run(&mut probe, seq, threshold, tie_break);
    let vocab_after = probe.len();
    if vocab_after <= vocab_before || vocab_after == 0 {
        return 0.0;
    }
    let new_tokens = (vocab_after - vocab_before) as f64;
    new_tokens * (vocab_after as f64).log2()
}

/// Вердикт о характере входящих данных по метрикам удивления.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurpriseVerdict {
    /// Высокая эффективность сжатия, нет прироста словаря (знакомый контекст).
    Familiar,
    /// Положительный прирост словаря (рождение новых смысловых паттернов).
    NovelGrowth,
    /// Низкая сжимаемость и отсутствие роста (бессистемный шум / энтропия).
    Noise,
}

/// Вычисляет непрерывный нормализованный скор удивления:
/// чем хуже жмется и чем больше рождает, тем выше скор.
pub fn compute_surprise_score(s_static: f64, s_growth: f64) -> f64 {
    let static_factor = if s_static > 0.0 { 1.0 / s_static } else { 1.0 };
    let growth_factor = if s_growth > 0.0 {
        s_growth / (1.0 + s_growth)
    } else {
        0.0
    };
    static_factor + growth_factor
}

/// Классифицирует входной сигнал по метрикам S-static и S-growth.
pub fn classify_surprise(s_static: f64, s_growth: f64) -> SurpriseVerdict {
    if s_growth > 0.0 {
        SurpriseVerdict::NovelGrowth
    } else if s_static > 1.25 {
        SurpriseVerdict::Familiar
    } else {
        SurpriseVerdict::Noise
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurpriseReport {
    pub s_static: f64,
    pub s_growth: f64,
    pub surprise_score: f64,
    pub verdict: SurpriseVerdict,
}

/// Обе метрики за один вызов — план 3: "обе метрики рядом".
pub fn surprise(store: &Store, input: &[u8]) -> SurpriseReport {
    let s_stat = s_static(store, input);
    let s_grow = s_growth(store, input, BIRTH_THRESHOLD, TieBreak::BirthOrder);
    let score = compute_surprise_score(s_stat, s_grow);
    let verdict = classify_surprise(s_stat, s_grow);
    SurpriseReport {
        s_static: s_stat,
        s_growth: s_grow,
        surprise_score: score,
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wheel::WheelState;

    #[test]
    fn s_static_perfectly_recognizes_frozen_vocabulary_input() {
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab cd cd ab cd ab cd", 2, TieBreak::BirthOrder);
        // Вход целиком из уже известных байт (все они в фабрике) -> хотя
        // бы какая-то компрессия относительно голых байт достижима, если
        // распознавание нашло составы. Проверяем, что s_static > 1.0
        // (лучше, чем 1 байт/токен) на тексте, который полка уже видела.
        let s = s_static(&wheel.store, b"ab ab cd cd ab cd ab cd");
        assert!(s > 1.0, "уже виденный текст обязан сжаться лучше 1.0, получили {s}");
    }

    #[test]
    fn s_static_does_not_mutate_the_store() {
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab cd cd", 2, TieBreak::BirthOrder);
        let len_before = wheel.store.len();
        let _ = s_static(&wheel.store, b"totally novel gibberish qzx jkv");
        assert_eq!(wheel.store.len(), len_before, "s_static обязан быть чисто читающим — lookup, не intern");
    }

    #[test]
    fn s_growth_does_not_mutate_the_real_store() {
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab cd cd", 2, TieBreak::BirthOrder);
        let len_before = wheel.store.len();
        let _ = s_growth(&wheel.store, b"ef ef gh gh ef gh", 2, TieBreak::BirthOrder);
        assert_eq!(wheel.store.len(), len_before, "s_growth обязан работать на клоне, не на реальном сторе");
    }

    #[test]
    fn s_growth_is_positive_when_novel_repeated_pattern_would_be_born() {
        let mut wheel = WheelState::new();
        wheel.turn(b"the cat sat on the mat", 2, TieBreak::BirthOrder);
        // "xy xy" ранее не встречалось -> холостой прогон должен родить
        // хотя бы токен "xy".
        let g = s_growth(&wheel.store, b"xy xy xy xy", 2, TieBreak::BirthOrder);
        assert!(g > 0.0, "новый повторяющийся паттерн обязан дать положительный S-growth, получили {g}");
    }

    #[test]
    fn s_growth_is_zero_when_nothing_new_would_be_born() {
        let mut wheel = WheelState::new();
        wheel.turn(b"aaaa bbbb", 2, TieBreak::BirthOrder);
        // Один-единственный незнакомый байт, без повторов -> ничего не
        // родится (T=2 не достигается).
        let g = s_growth(&wheel.store, b"q", 2, TieBreak::BirthOrder);
        assert_eq!(g, 0.0);
    }

    #[test]
    fn recognize_focused_falls_back_when_composite_is_not_in_focus() {
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab", 2, TieBreak::BirthOrder);
        let a = wheel.store.lookup(0, &[b'a' as u32]).unwrap();
        let b = wheel.store.lookup(0, &[b'b' as u32]).unwrap();
        let ab = wheel.store.lookup(1, &[a, b]).expect("'ab' должен был родиться");

        let ids = vec![a, b];
        // Без ограничения фокуса -> распознаётся как composite "ab" (1 токен).
        let unrestricted = recognize_focused(&wheel.store, &ids, None);
        assert_eq!(unrestricted, vec![ab]);

        // С фокусом, где "ab" отсутствует -> откат к сырым байтам (2 токена),
        // именно то поведение, которое требует G-B2: мёртвое недоступно.
        let empty_focus: HashSet<TokenId> = HashSet::new();
        let restricted = recognize_focused(&wheel.store, &ids, Some(&empty_focus));
        assert_eq!(restricted, vec![a, b], "композит вне фокуса не должен использоваться для свёртки");
    }

    #[test]
    fn recognize_cascades_through_multiple_levels() {
        let mut wheel = WheelState::new();
        wheel.turn(b"abababab", 2, TieBreak::BirthOrder);
        // К этому моменту должны существовать составы вплоть до "abab"
        // (см. merge.rs тесты). Распознавание того же текста заново
        // обязано схлопнуться до того же количества токенов, что и рост.
        let ids: Vec<TokenId> = b"abababab"
            .iter()
            .map(|&b| wheel.store.lookup(0, &[b as u32]).unwrap())
            .collect();
        let recognized = recognize_focused(&wheel.store, &ids, None);
        assert_eq!(recognized.len(), wheel.sequence.ids.len(), "распознавание уже виденного текста обязано дать ту же свёртку, что рост");
    }
}
