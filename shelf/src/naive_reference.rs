//! Прямой порт python-эталона (arch/этап 1/axmv2.py, `librarian_iteration`
//! + `run_librarian`) — полный пересчёт пар на каждой итерации, без кучи и
//! связного списка. Существует только для теста эквивалентности с новым
//! O(n log n) алгоритмом (merge.rs) на маленьком входе, до применения
//! последнего на реальном корпусе (DEV_GUIDE, правило 11). Не для
//! использования вне тестов — не оптимизировать, не выносить в публичный
//! API.

#![cfg(test)]

use crate::store::{Store, TokenId};
use std::collections::HashMap;

fn iteration(store: &mut Store, sequence: &[TokenId], threshold: u64) -> (Vec<TokenId>, usize) {
    let n = sequence.len();
    let mut pair_positions: HashMap<(TokenId, TokenId), Vec<usize>> = HashMap::new();
    for i in 0..n.saturating_sub(1) {
        pair_positions
            .entry((sequence[i], sequence[i + 1]))
            .or_default()
            .push(i);
    }

    let mut candidates: Vec<(TokenId, TokenId)> = pair_positions
        .iter()
        .filter(|(_, pos)| pos.len() as u64 >= threshold)
        .map(|(&p, _)| p)
        .collect();
    // (-count, a, b) по возрастанию a,b — тот же тайбрейк, что в python.
    candidates.sort_by(|a, b| {
        let ca = pair_positions[a].len();
        let cb = pair_positions[b].len();
        cb.cmp(&ca).then_with(|| a.cmp(b))
    });

    let mut seq = sequence.to_vec();
    let mut alive = vec![true; n];
    let mut births = 0usize;

    for (a, b) in candidates {
        let level = store.level_of(a).max(store.level_of(b)) + 1;
        let before = store.birth_log().len();
        let new_id = store.intern(level, &[a, b]);
        if store.birth_log().len() > before {
            births += 1;
        }
        for &i in &pair_positions[&(a, b)] {
            if i + 1 < n && alive[i] && alive[i + 1] && seq[i] == a && seq[i + 1] == b {
                seq[i] = new_id;
                alive[i + 1] = false;
                store.touch(new_id);
            }
        }
    }

    (
        (0..n).filter(|&i| alive[i]).map(|i| seq[i]).collect(),
        births,
    )
}

pub fn run(store: &mut Store, sequence: Vec<TokenId>, threshold: u64, max_iterations: usize) -> Vec<TokenId> {
    let mut seq = sequence;
    for _ in 0..max_iterations {
        let (next_seq, births) = iteration(store, &seq, threshold);
        seq = next_seq;
        if births == 0 {
            break;
        }
    }
    seq
}
