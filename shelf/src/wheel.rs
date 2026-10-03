//! Шаг 12 (план 3): Колесо — оборот как единица времени системы.
//!
//! **Определение (план 3, принято):** один оборот = одна полная итерация
//! цикла «приём порции потока → волна слияний → запекание». Калибровка
//! порции: 64 КБ входа на оборот; корпус Гиты (2.8 МБ) ≈ 44 оборота за
//! полный прогон.
//!
//! **Р4 (время только на границе):** временную метку (номер оборота)
//! штампует только Колесо, на входе новой порции. Внутри системы «когда»
//! = «в каком обороте», никогда не «в котором часу» — оборот заменяет
//! часы как единственная единица времени, видимая механизму.
//!
//! **Р5 (атомарность оборота):** внешние события (новая порция потока)
//! принимаются только на границе между оборотами — оборот, начавшись, не
//! прерывается. Здесь это буквально: `turn()` не возвращает управление,
//! пока волна слияний для этой порции не сойдётся.
//!
//! **Архитектурное решение, не покрытое буквой плана явно (DEV_GUIDE,
//! правило 2):** реализация оборота требует РЕАЛЬНОЙ порционной подачи
//! потока — нельзя вывести номера оборотов постфактум из одноразового
//! прогона `merge::run` на всём корпусе разом, потому что «в каком
//! обороте токен последний раз был в свёртке» зависит от того, что
//! ИМЕННО было в `out.ids` в момент завершения каждой конкретной порции,
//! а это состояние существует только если порции действительно
//! подавались по очереди. Отсюда `WheelState` — держит растущую
//! последовательность между оборотами, `turn()` дозаписывает новую
//! порцию байт и заново прогоняет `merge::run` на всей текущей
//! последовательности (уже свёрнутый префикс просто не даст новых
//! слияний внутри себя — это не расточительство корректности, это
//! наблюдаемая скорость; DEV_GUIDE п.10, диагностировать, если станет
//! реальной проблемой, не оптимизировать заранее).

use crate::merge::{self, Sequence, TieBreak};
use crate::store::{Store, TokenId};
use crate::surprise::{classify_surprise, compute_surprise_score, s_static, SurpriseVerdict};
use std::collections::HashMap;

pub const REVOLUTION_CHUNK_BYTES: usize = 64 * 1024;

/// Отчет об обороте Колеса: параметры новизны и удивления входящей порции потока.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnReport {
    pub revolution: u32,
    pub s_static: f64,
    pub s_growth: f64,
    pub surprise_score: f64,
    pub verdict: SurpriseVerdict,
    pub new_tokens_count: usize,
    pub sequence_len: usize,
}

pub struct WheelState {
    pub store: Store,
    pub sequence: Sequence,
    /// Номер последнего завершённого оборота (0 — ничего ещё не подано).
    pub revolution: u32,
    /// token -> номер оборота, на выходе которого токен последний раз
    /// был элементом `sequence.ids` (свёртки потока). НЕ считает
    /// вхождение токена как чужого ребёнка — только прямое присутствие
    /// в текущей свёрнутой последовательности (шаг 12: "входящий в
    /// свёртку потока").
    pub last_touched_revolution: HashMap<TokenId, u32>,
}

impl WheelState {
    pub fn new() -> Self {
        let mut store = Store::new();
        store.init_factory();
        Self::with_store(store)
    }

    pub fn with_store(store: Store) -> Self {
        WheelState {
            store,
            sequence: Sequence { ids: Vec::new(), levels: Vec::new() },
            revolution: 0,
            last_touched_revolution: HashMap::new(),
        }
    }

    /// Один оборот: принять порцию байт, дописать в конец текущей
    /// последовательности, прогнать волну слияний на всей
    /// последовательности целиком. Атомарно (Р5).
    ///
    /// Синхронно вычисляет метрики Surprise в горячем цикле:
    /// - `s_static` замеряется на замороженном сторе ДО мутаций;
    /// - `s_growth` вычисляется по реальному приросту словаря БЕЗ клонирования стора.
    pub fn turn(&mut self, chunk: &[u8], threshold: u64, tie_break: TieBreak) -> TurnReport {
        // 1. Замер S-static на замороженном сторе ДО мутаций оборота
        let s_static = s_static(&self.store, chunk);
        let vocab_before = self.store.len();

        let mut before_counts: HashMap<TokenId, u64> = HashMap::new();
        for &id in &self.sequence.ids {
            before_counts.entry(id).or_insert_with(|| self.store.count_of(id));
        }

        let mut ids = std::mem::take(&mut self.sequence.ids);
        ids.reserve(chunk.len());
        for &b in chunk {
            let id = self.store.intern(0, &[b as u32]);
            self.store.touch(id);
            ids.push(id);
        }

        let seq = Sequence::from_ids(&self.store, ids);
        let out = merge::run(&mut self.store, seq, threshold, tie_break);

        self.revolution += 1;
        for &id in &out.ids {
            let touched_this_turn = match before_counts.get(&id) {
                Some(&old_count) => self.store.count_of(id) > old_count,
                None => true, // не было в перенесённом префиксе -> новое в этом обороте
            };
            if touched_this_turn {
                self.last_touched_revolution.insert(id, self.revolution);
            }
        }
        
        let mut final_ids = out.ids;
        if final_ids.len() > 100_000 {
            let excess = final_ids.len() - 100_000;
            final_ids.drain(0..excess);
        }
        
        let final_levels = final_ids.iter().map(|&id| self.store.level_of(id)).collect();
        self.sequence = Sequence { ids: final_ids, levels: final_levels };

        // 2. Расчет S-growth по реально рожденным токенам без клонирования стора
        let vocab_after = self.store.len();
        let new_tokens_count = vocab_after.saturating_sub(vocab_before);
        let s_growth = if new_tokens_count > 0 {
            (new_tokens_count as f64) * (vocab_after as f64).log2()
        } else {
            0.0
        };

        let surprise_score = compute_surprise_score(s_static, s_growth);
        let verdict = classify_surprise(s_static, s_growth);

        TurnReport {
            revolution: self.revolution,
            s_static,
            s_growth,
            surprise_score,
            verdict,
            new_tokens_count,
            sequence_len: self.sequence.ids.len(),
        }
    }

    /// Прогнать весь корпус оборотами по `REVOLUTION_CHUNK_BYTES` (план 3:
    /// 64 КБ). Возвращает число оборотов (для сверки с калибровкой плана
    /// — ~44 на bhagavad_gita.txt).
    pub fn run_corpus(&mut self, bytes: &[u8], threshold: u64, tie_break: TieBreak) -> u32 {
        for chunk in bytes.chunks(REVOLUTION_CHUNK_BYTES) {
            self.turn(chunk, threshold, tie_break);
        }
        self.revolution
    }
}

impl Default for WheelState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revolution_count_matches_chunking() {
        let mut wheel = WheelState::new();
        // 3 порции: ровно 64КБ, 64КБ, и 1 байт хвоста -> 3 оборота.
        let bytes = vec![b'a'; REVOLUTION_CHUNK_BYTES * 2 + 1];
        let revolutions = wheel.run_corpus(&bytes, 2, TieBreak::BirthOrder);
        assert_eq!(revolutions, 3);
        assert_eq!(wheel.revolution, 3);
    }

    #[test]
    fn gita_calibration_is_approximately_44_revolutions() {
        // План 3 округляет размер файла до "2.8 МБ" — точный размер
        // (2 814 658 байт) даёт 43 оборота, план говорит "≈ 44": это
        // округление калибровки, не точное число, допуск ±1.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let corpus_path = std::path::Path::new(manifest_dir).join("../docs/corpus/bhagavad_gita.txt");
        let bytes = std::fs::read(&corpus_path).expect("bhagavad_gita.txt");
        let expected = bytes.len().div_ceil(REVOLUTION_CHUNK_BYTES);
        assert!((43..=44).contains(&expected), "калибровка плана 3: 2.8 МБ / 64 КБ ≈ 44 оборота, получили {expected}");
    }

    #[test]
    fn sequence_grows_and_collapses_across_revolutions() {
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab cd cd ab cd ab cd ", 2, TieBreak::BirthOrder);
        let after_first = wheel.sequence.ids.len();
        assert!(after_first > 0);
        wheel.turn(b"ab ab cd cd ab cd ab cd ", 2, TieBreak::BirthOrder);
        // Вторая порция должна была слиться с уже свёрнутым хвостом
        // первой (или как минимум не создать наивный необработанный
        // остаток) -- проверяем, что последовательность не выросла ровно
        // на длину новой порции в байтах (то есть слияние реально
        // произошло, а не просто конкатенация).
        assert!(wheel.sequence.ids.len() < after_first + 24, "вторая порция обязана частично слиться, не просто дописаться");
    }

    #[test]
    fn last_touched_revolution_tracks_direct_fold_presence() {
        let mut wheel = WheelState::new();
        wheel.turn(b"xyz", 2, TieBreak::BirthOrder);
        assert_eq!(wheel.revolution, 1);
        for &id in &wheel.sequence.ids {
            assert_eq!(wheel.last_touched_revolution.get(&id), Some(&1));
        }
    }

    #[test]
    fn stale_token_sitting_unchanged_does_not_get_timestamp_refreshed() {
        // "ab" сливается в оборот 1. Последующие обороты с несвязанным
        // "zzz" не должны переписывать last_touched_revolution["ab"],
        // даже если "ab" физически продолжает стоять в растущей
        // последовательности (она не забывает старое). Иначе K-оборотное
        // окно (шаг 12) никогда бы никого не устаревало.
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab", 2, TieBreak::BirthOrder);
        let ab = wheel
            .sequence
            .ids
            .iter()
            .copied()
            .find(|&id| wheel.store.bytes_of(id) == b"ab")
            .expect("'ab' должен был слиться в композит");
        assert_eq!(wheel.last_touched_revolution.get(&ab), Some(&1));

        wheel.turn(b"zzz", 2, TieBreak::BirthOrder);
        wheel.turn(b"zzz", 2, TieBreak::BirthOrder);
        assert!(
            wheel.sequence.ids.contains(&ab),
            "'ab' обязан физически оставаться в последовательности (она не забывает старое)"
        );
        assert_eq!(
            wheel.last_touched_revolution.get(&ab),
            Some(&1),
            "штамп времени не должен переписываться, пока токен просто лежит нетронутым"
        );
    }
}
