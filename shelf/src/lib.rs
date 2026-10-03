pub mod context;
pub mod cooc;
pub mod death;
pub mod inverted;
pub mod merge;
pub mod ppmi;
#[cfg(test)]
mod naive_reference;
pub mod persist;
pub mod rng;
pub mod second_shelf;
pub mod store;
pub mod surprise;
pub mod walks;
pub mod wheel;
pub mod port;
pub mod recall;
pub mod recognizer;

pub use death::{DeathLog, FocusConcept, FocusEvent, FocusSet, MarkOutcome, WeightCutOutcome};
pub use merge::Sequence;
pub use persist::{load, load_all, save, save_all, LoadError};
pub use recall::{recall, recall_from_csr, RecallCandidate, RecallConfig, RecallResult};
pub use recognizer::StreamingRecognizer;
pub use store::{Store, TokenId};
pub use surprise::{
    classify_surprise, compute_surprise_score, s_growth, s_static, s_static_focused, surprise,
    SurpriseReport, SurpriseVerdict,
};
pub use wheel::{TurnReport, WheelState, REVOLUTION_CHUNK_BYTES};

/// Порог рождения пары (T в python-эталоне). Значение унаследовано из
/// принятой спеки этапа 1 (arch/этап 1/axmv2.py) — не варьировать тихо.
pub const BIRTH_THRESHOLD: u64 = 2;

/// Шаг 1 первого прохода: прочитать корпус, интернировать байты уровня 0,
/// потрогать счётчик каждого вхождения. Прямой порт
/// `step1_read_and_touch` из python-эталона.
pub fn read_and_touch(store: &mut Store, corpus_bytes: &[u8]) -> Vec<TokenId> {
    corpus_bytes
        .iter()
        .map(|&b| {
            let id = store.intern(0, &[b as u32]);
            store.touch(id);
            id
        })
        .collect()
}

#[cfg(test)]
mod equivalence {
    //! Сверка нового O(n log n) алгоритма (merge::run) с прямым портом
    //! python-эталона (naive_reference::run) на маленьком входе — до
    //! применения на реальном корпусе. DEV_GUIDE, правило 11.
    use super::*;
    use std::collections::HashSet;

    fn distinct_fragments_at_or_above(store: &Store, threshold: u64) -> HashSet<Vec<u8>> {
        (0..store.len() as TokenId)
            .filter(|&id| store.level_of(id) >= 1 && store.count_of(id) >= threshold)
            .map(|id| store.bytes_of(id))
            .collect()
    }

    fn run_new(corpus: &[u8]) -> (Store, Vec<TokenId>) {
        let mut store = Store::new();
        store.init_factory();
        let ids = read_and_touch(&mut store, corpus);
        let seq = Sequence::from_ids(&store, ids);
        let out = merge::run(&mut store, seq, BIRTH_THRESHOLD, merge::TieBreak::BirthOrder);
        (store, out.ids)
    }

    fn run_naive(corpus: &[u8]) -> (Store, Vec<TokenId>) {
        let mut store = Store::new();
        store.init_factory();
        let ids = read_and_touch(&mut store, corpus);
        let out = naive_reference::run(&mut store, ids, BIRTH_THRESHOLD, 200);
        (store, out)
    }

    #[test]
    fn agrees_with_naive_reference_on_repeated_natural_text() {
        let corpus =
            b"the cat sat on the mat. the cat sat on the mat. the cat ran to the mat.";

        let (new_store, new_seq) = run_new(corpus);
        let (naive_store, naive_seq) = run_naive(corpus);

        assert_eq!(new_seq.len(), naive_seq.len(), "разная длина итоговой последовательности");

        let new_fragments = distinct_fragments_at_or_above(&new_store, BIRTH_THRESHOLD);
        let naive_fragments = distinct_fragments_at_or_above(&naive_store, BIRTH_THRESHOLD);
        assert_eq!(
            new_fragments, naive_fragments,
            "разный словарь составных токенов при пороге T={}",
            BIRTH_THRESHOLD
        );
    }

    #[test]
    fn agrees_with_naive_reference_on_short_repetitive_run() {
        // Короткий вход без тройных перекрытий одного байта подряд —
        // область, где формально доказанной эквивалентности достаточно.
        let corpus = b"ab ab cd cd ab cd ab cd";
        let (new_store, new_seq) = run_new(corpus);
        let (naive_store, naive_seq) = run_naive(corpus);
        assert_eq!(new_seq.len(), naive_seq.len());
        assert_eq!(
            distinct_fragments_at_or_above(&new_store, BIRTH_THRESHOLD),
            distinct_fragments_at_or_above(&naive_store, BIRTH_THRESHOLD)
        );
    }
}
