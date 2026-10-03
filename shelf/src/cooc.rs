//! Шаг 9 (A2, план 3): накопление со-встречаемости (COO) и запекание в CSR.
//!
//! COO — растущий hashmap-аккумулятор, естественная форма накопления
//! (вставка/суммирование O(1) амортизированно, порядок поступления
//! событий не важен — суммирование коммутативно). CSR — испечённая форма
//! для чтения: перекладка COO→CSR происходит внутри операции "сна"
//! (план 3 §9: "это и есть первый кусок механики запекания второй
//! полки"). Не путать с нормализацией — это раскладка на скорость,
//! денормализующая по своей природе (блокнот 3, §10).
//!
//! Детерминизм (Р2) в запекании — не свойство HashMap (порядок обхода не
//! гарантирован), а явная сортировка на выходе: строки CSR отсортированы
//! по TokenId, колонки внутри строки — тоже. Два запекания одного и того
//! же накопленного состояния обязаны дать побитово идентичный CSR
//! независимо от порядка, в котором события были накоплены.
//!
//! Count-min sketch НЕ используется (явное решение плана 3, шаг 9):
//! ошибка CMS односторонняя (только завышение) и бьёт сильнее всего по
//! редким парам — которые для PMI (шаг 11) важнее всего. Включать только
//! при нехватке памяти, и только вместе с частотным полом + conservative
//! update — не сейчас.
//!
//! Персист CSR-снапшота в redb здесь НЕ реализован: план называет формат
//! снапшота частью golden-fixture (Р2), но не просит персист в этом шаге
//! отдельно от механики запекания — решение не заводить его "про запас"
//! (DEV_GUIDE, правило 5), появится вместе с golden-fixture, когда до неё
//! дойдёт очередь.

use crate::store::TokenId;
use std::collections::HashMap;

/// Растущий аккумулятор со-встречаемости. Одна запись на пару (token,
/// context) — сумма весов всех событий этой пары, вне зависимости от
/// того, сколько раз и в каком порядке `accumulate` вызывался.
#[derive(Default)]
pub struct CooAccumulator {
    counts: HashMap<(TokenId, TokenId), u64>,
}

impl CooAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Добавить поток событий экстрактора (см. context.rs). Порядок
    /// вызовов и порядок событий внутри потока не влияют на итоговые
    /// счётчики — суммирование коммутативно.
    pub fn accumulate(&mut self, events: &[(TokenId, TokenId, u64)]) {
        for &(token, context, weight) in events {
            *self.counts.entry((token, context)).or_insert(0) += weight;
        }
    }

    pub fn total_weight(&self) -> u64 {
        self.counts.values().sum()
    }

    pub fn distinct_pairs(&self) -> usize {
        self.counts.len()
    }

    pub fn weight_of(&self, token: TokenId, context: TokenId) -> u64 {
        self.counts.get(&(token, context)).copied().unwrap_or(0)
    }

    /// Испечь в CSR: строки — токены с хотя бы одной связью,
    /// отсортированы по возрастанию; колонки внутри строки — тоже.
    /// Заодно считает маргиналы (частоты контекстов — знаменатель PMI,
    /// план 3 §9: "счётчики частот контекстов там же").
    pub fn bake_to_csr(&self) -> Csr {
        let mut by_row: HashMap<TokenId, Vec<(TokenId, u64)>> = HashMap::new();
        let mut row_marginal: HashMap<TokenId, u64> = HashMap::new();
        let mut col_marginal: HashMap<TokenId, u64> = HashMap::new();
        let mut total_weight: u64 = 0;

        for (&(token, context), &weight) in self.counts.iter() {
            by_row.entry(token).or_default().push((context, weight));
            *row_marginal.entry(token).or_insert(0) += weight;
            *col_marginal.entry(context).or_insert(0) += weight;
            total_weight += weight;
        }

        let mut row_index: Vec<TokenId> = by_row.keys().copied().collect();
        row_index.sort_unstable();

        let mut row_ptr: Vec<u32> = Vec::with_capacity(row_index.len() + 1);
        let mut col_idx: Vec<TokenId> = Vec::new();
        let mut values: Vec<u64> = Vec::new();

        row_ptr.push(0);
        for &row in &row_index {
            let mut entries = by_row.remove(&row).expect("row_index построен из ключей by_row");
            entries.sort_unstable_by_key(|&(ctx, _)| ctx);
            for (ctx, w) in entries {
                col_idx.push(ctx);
                values.push(w);
            }
            row_ptr.push(col_idx.len() as u32);
        }

        Csr {
            row_index,
            row_ptr,
            col_idx,
            values,
            row_marginal,
            col_marginal,
            total_weight,
        }
    }
}

/// Compressed Sparse Row — испечённая, только для чтения форма
/// со-встречаемости. `row_index[i]..` описывает диапазон
/// `row_ptr[i]..row_ptr[i+1]` в `col_idx`/`values` для строки `i`.
pub struct Csr {
    pub row_index: Vec<TokenId>,
    pub row_ptr: Vec<u32>,
    pub col_idx: Vec<TokenId>,
    pub values: Vec<u64>,
    /// row_marginal[t] = сумма весов всех связей (t, *) — p(a) числитель.
    pub row_marginal: HashMap<TokenId, u64>,
    /// col_marginal[t] = сумма весов всех связей (*, t) — p(b) числитель.
    pub col_marginal: HashMap<TokenId, u64>,
    pub total_weight: u64,
}

impl Csr {
    /// Срез (context, weight) для данного токена-строки, отсортированный
    /// по context. Пусто, если у токена нет исходящих связей.
    pub fn row(&self, token: TokenId) -> impl Iterator<Item = (TokenId, u64)> + '_ {
        let range = match self.row_index.binary_search(&token) {
            Ok(i) => self.row_ptr[i] as usize..self.row_ptr[i + 1] as usize,
            Err(_) => 0..0,
        };
        self.col_idx[range.clone()]
            .iter()
            .copied()
            .zip(self.values[range].iter().copied())
    }

    pub fn weight_of(&self, token: TokenId, context: TokenId) -> u64 {
        self.row(token)
            .find(|&(ctx, _)| ctx == context)
            .map(|(_, w)| w)
            .unwrap_or(0)
    }

    pub fn row_count(&self) -> usize {
        self.row_index.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulate_sums_repeated_pairs() {
        let mut acc = CooAccumulator::new();
        acc.accumulate(&[(1, 2, 3), (1, 2, 4), (2, 3, 1)]);
        assert_eq!(acc.weight_of(1, 2), 7);
        assert_eq!(acc.weight_of(2, 3), 1);
        assert_eq!(acc.distinct_pairs(), 2);
        assert_eq!(acc.total_weight(), 8);
    }

    #[test]
    fn bake_matches_hand_computed_csr() {
        let mut acc = CooAccumulator::new();
        acc.accumulate(&[(1, 2, 5), (1, 3, 2), (2, 1, 1)]);
        let csr = acc.bake_to_csr();

        assert_eq!(csr.row_index, vec![1, 2]);
        let row1: Vec<(TokenId, u64)> = csr.row(1).collect();
        assert_eq!(row1, vec![(2, 5), (3, 2)], "колонки внутри строки отсортированы");
        let row2: Vec<(TokenId, u64)> = csr.row(2).collect();
        assert_eq!(row2, vec![(1, 1)]);
        assert_eq!(csr.row(99).count(), 0, "токен без связей -> пустой ряд, не паника");
    }

    #[test]
    fn bake_marginals_and_total_match_brute_force() {
        let mut acc = CooAccumulator::new();
        acc.accumulate(&[(1, 2, 5), (1, 3, 2), (2, 1, 1), (3, 1, 4)]);
        let csr = acc.bake_to_csr();

        assert_eq!(csr.row_marginal[&1], 7); // (1,2,5)+(1,3,2)
        assert_eq!(csr.row_marginal[&2], 1);
        assert_eq!(csr.row_marginal[&3], 4);
        assert_eq!(csr.col_marginal[&2], 5);
        assert_eq!(csr.col_marginal[&3], 2);
        assert_eq!(csr.col_marginal[&1], 5); // (2,1,1)+(3,1,4)
        assert_eq!(csr.total_weight, acc.total_weight());
        assert_eq!(csr.values.iter().sum::<u64>(), acc.total_weight(), "сумма COO == сумма CSR (стоп-условие шага 9)");
    }

    #[test]
    fn bake_is_deterministic_regardless_of_accumulation_order() {
        let events_forward: Vec<(TokenId, TokenId, u64)> =
            vec![(5, 1, 2), (1, 5, 3), (3, 3, 1), (5, 1, 4), (2, 8, 1)];
        let mut events_shuffled = events_forward.clone();
        events_shuffled.reverse();
        events_shuffled.swap(0, 2);

        let mut acc_a = CooAccumulator::new();
        for e in &events_forward {
            acc_a.accumulate(std::slice::from_ref(e));
        }
        let mut acc_b = CooAccumulator::new();
        for e in &events_shuffled {
            acc_b.accumulate(std::slice::from_ref(e));
        }

        let csr_a = acc_a.bake_to_csr();
        let csr_b = acc_b.bake_to_csr();

        assert_eq!(csr_a.row_index, csr_b.row_index);
        assert_eq!(csr_a.row_ptr, csr_b.row_ptr);
        assert_eq!(csr_a.col_idx, csr_b.col_idx);
        assert_eq!(csr_a.values, csr_b.values);
    }

    #[test]
    fn baking_twice_from_same_accumulator_is_bit_identical() {
        let mut acc = CooAccumulator::new();
        acc.accumulate(&[(1, 2, 5), (1, 3, 2), (2, 1, 1), (3, 1, 4), (7, 7, 9)]);
        let csr1 = acc.bake_to_csr();
        let csr2 = acc.bake_to_csr();
        assert_eq!(csr1.row_index, csr2.row_index);
        assert_eq!(csr1.row_ptr, csr2.row_ptr);
        assert_eq!(csr1.col_idx, csr2.col_idx);
        assert_eq!(csr1.values, csr2.values);
    }

    #[test]
    fn real_corpus_coo_bake_roundtrip_sum_matches() {
        use crate::context::{ContextExtractor, Neighbor, Parent};
        use crate::merge::{self, Sequence, TieBreak};
        use crate::{read_and_touch, Store, BIRTH_THRESHOLD};
        use std::path::Path;

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let corpus_path = Path::new(manifest_dir).join("../docs/corpus/Стихи 2025.md");
        let bytes = std::fs::read(&corpus_path).expect("Стихи 2025.md должен быть доступен");

        let mut store = Store::new();
        store.init_factory();
        let ids = read_and_touch(&mut store, &bytes);
        let seq = Sequence::from_ids(&store, ids);
        let out = merge::run(&mut store, seq, BIRTH_THRESHOLD, TieBreak::BirthOrder);

        let mut acc = CooAccumulator::new();
        acc.accumulate(&Neighbor.extract(&store, &out.ids));
        acc.accumulate(&Parent.extract(&store, &out.ids));
        assert!(acc.distinct_pairs() > 0);

        let csr = acc.bake_to_csr();
        assert_eq!(csr.values.iter().sum::<u64>(), acc.total_weight());

        let csr2 = acc.bake_to_csr();
        assert_eq!(csr.row_ptr, csr2.row_ptr, "запекание детерминировано на реальном корпусе");
        assert_eq!(csr.col_idx, csr2.col_idx);
        assert_eq!(csr.values, csr2.values);
    }
}
