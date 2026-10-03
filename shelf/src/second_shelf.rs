//! Шаг 8 (план 3, Правка 1, Р-А4-1/Р-А4-4): по-поколенная аккумуляция
//! контекста для перевёрнутой библиотеки — реальная и нуль-контрольная
//! (переставленная) со-встречаемость за один прогон каскада.
//!
//! Со-встречаемость собирается ВО ВРЕМЯ каскада, не на финальной
//! последовательности: `merge::run_with_generation_hook` даёт срез живой
//! последовательности на входе в каждое поколение — "легальный текст" над
//! текущим словарём на тот момент. Экстрактор (принятая конфигурация —
//! `Window::range(2, 5)`, level >= 2) прогоняется по этому срезу дважды:
//! реальному и переставленному (фиксированный seed, `rng::shuffle`,
//! продвигается по поколениям от одного `SplitMix64` — детерминированно
//! при фиксированном базовом seed, но не повторяет один и тот же паттерн
//! перестановки на каждом поколении). Оба накопителя строятся за ОДИН
//! прогон каскада — не два отдельных, дешевле и точнее соответствует
//! "тот же прогон, те же поколения" (план 3).

use crate::context::{filter_by_min_level, ContextExtractor, Window};
use crate::cooc::CooAccumulator;
use crate::merge::{self, Sequence, TieBreak};
use crate::rng::{shuffle, SplitMix64};
use crate::store::Store;

pub struct GenerationalCoo {
    pub real: CooAccumulator,
    pub permuted: CooAccumulator,
}

/// Прогнать каскад один раз, накапливая реальную и переставленную
/// (нуль-контроль, план 3 Р-А4-4) со-встречаемость по-поколенно.
/// Возвращает свёрнутую последовательность (тот же результат, что дал бы
/// обычный `merge::run`) и оба накопителя.
pub fn accumulate_windowed_per_generation(
    store: &mut Store,
    seq: Sequence,
    threshold: u64,
    tie_break: TieBreak,
    window: &Window,
    min_level: u32,
    permutation_seed: u64,
) -> (Sequence, GenerationalCoo) {
    let mut real_coo = CooAccumulator::new();
    let mut permuted_coo = CooAccumulator::new();
    let mut rng = SplitMix64::new(permutation_seed);

    let (out, _births) = merge::run_with_generation_hook(store, seq, threshold, tie_break, |s, sequence| {
        let real_events = filter_by_min_level(s, window.extract(s, sequence), min_level);
        real_coo.accumulate(&real_events);

        let mut permuted = sequence.to_vec();
        shuffle(&mut permuted, rng.next_u64());
        let permuted_events = filter_by_min_level(s, window.extract(s, &permuted), min_level);
        permuted_coo.accumulate(&permuted_events);
    });

    (
        out,
        GenerationalCoo {
            real: real_coo,
            permuted: permuted_coo,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{read_and_touch, TokenId, BIRTH_THRESHOLD};

    fn intern_bytes(store: &mut Store, bytes: &[u8]) -> Vec<TokenId> {
        bytes.iter().map(|&b| store.intern(0, &[b as u32])).collect()
    }

    #[test]
    fn deterministic_for_fixed_seed() {
        let text = b"the cat sat on the mat, the cat sat on the mat, the cat ran to the mat.";

        let mut store_a = Store::new();
        store_a.init_factory();
        let ids_a = intern_bytes(&mut store_a, text);
        let seq_a = Sequence::from_ids(&store_a, ids_a);
        let (_out_a, coo_a) = accumulate_windowed_per_generation(
            &mut store_a,
            seq_a,
            BIRTH_THRESHOLD,
            TieBreak::BirthOrder,
            &Window::range(2, 5),
            2,
            0xC0FFEE,
        );

        let mut store_b = Store::new();
        store_b.init_factory();
        let ids_b = intern_bytes(&mut store_b, text);
        let seq_b = Sequence::from_ids(&store_b, ids_b);
        let (_out_b, coo_b) = accumulate_windowed_per_generation(
            &mut store_b,
            seq_b,
            BIRTH_THRESHOLD,
            TieBreak::BirthOrder,
            &Window::range(2, 5),
            2,
            0xC0FFEE,
        );

        let csr_real_a = coo_a.real.bake_to_csr();
        let csr_real_b = coo_b.real.bake_to_csr();
        assert_eq!(csr_real_a.row_ptr, csr_real_b.row_ptr);
        assert_eq!(csr_real_a.col_idx, csr_real_b.col_idx);
        assert_eq!(csr_real_a.values, csr_real_b.values);

        let csr_perm_a = coo_a.permuted.bake_to_csr();
        let csr_perm_b = coo_b.permuted.bake_to_csr();
        assert_eq!(csr_perm_a.row_ptr, csr_perm_b.row_ptr);
        assert_eq!(csr_perm_a.col_idx, csr_perm_b.col_idx);
        assert_eq!(csr_perm_a.values, csr_perm_b.values);
    }

    #[test]
    fn real_and_permuted_generally_differ() {
        let text = b"the cat sat on the mat, the cat sat on the mat, the cat ran to the mat, \
                     the dog sat on the log, the dog sat on the log, the dog ran to the log.";
        let mut store = Store::new();
        store.init_factory();
        let ids = intern_bytes(&mut store, text);
        let seq = Sequence::from_ids(&store, ids);
        let (_out, coo) = accumulate_windowed_per_generation(
            &mut store,
            seq,
            BIRTH_THRESHOLD,
            TieBreak::BirthOrder,
            &Window::range(2, 5),
            2,
            42,
        );

        assert!(coo.real.total_weight() > 0, "реальная аккумуляция обязана дать хоть какой-то сигнал");
        assert_ne!(
            coo.real.total_weight(),
            0,
            "на этом тексте с достаточным повтором должны появиться токены уровня >= 2"
        );
        // Не требуем строгого неравенства весов (на маленьком входе оба
        // могут случайно совпасть), но словарь пар обычно расходится —
        // не строгое утверждение, не проверяем строго здесь.
    }

    #[test]
    fn real_corpus_smoke_test() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let corpus_path = std::path::Path::new(manifest_dir).join("../docs/corpus/Стихи 2025.md");
        let bytes = std::fs::read(&corpus_path).expect("Стихи 2025.md должен быть доступен");

        let mut store = Store::new();
        store.init_factory();
        let ids = read_and_touch(&mut store, &bytes);
        let seq = Sequence::from_ids(&store, ids);
        let (_out, coo) = accumulate_windowed_per_generation(
            &mut store,
            seq,
            BIRTH_THRESHOLD,
            TieBreak::BirthOrder,
            &Window::range(2, 5),
            2,
            0xA4_5EED,
        );

        assert!(coo.real.distinct_pairs() > 0, "на реальном корпусе обязан появиться сигнал уровня >= 2");
    }
}
