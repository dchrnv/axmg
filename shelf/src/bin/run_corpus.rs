//! Прогон на реальных корпусах — подготовка снапшота для сверки на шаге 4.
//! Сам стоп-гейт (приёмка результата) — за человеком (DEV_GUIDE, правило
//! 7); эта программа только считает и печатает числа, ничего не решает.
//!
//! Дополнительно: диагностический прогон обоих вариантов TieBreak
//! (BirthOrder — принятый в шаге 3, ContentHash — как у python), чтобы
//! проверить гипотезу "разрыв словаря со словарём python объясняется
//! компаратором тай-брейка волны" экспериментом, а не декларацией.
//!
//! Пути — константы, правятся руками между прогонами (как в python-эталоне,
//! spec п.0.4: без CLI-аргументов).

use axmg::merge::TieBreak;
use axmg::{merge, read_and_touch, Sequence, Store, TokenId, BIRTH_THRESHOLD};
use std::collections::HashMap;
use std::path::Path;

const STIHI: &str = "../docs/corpus/Стихи 2025.md";
const GITA: &str = "../docs/corpus/bhagavad_gita.txt";

struct RunResult {
    store: Store,
    final_len: usize,
    original_bytes: usize,
    births_per_generation: Vec<usize>,
}

/// Известный эталон python (arch/этап 1/out/run2_bhagavad_step3/snapshot.json,
/// поле births_per_iteration) — для прямой поколение-к-поколению сверки,
/// не только агрегатов.
const PYTHON_GITA_BIRTHS: &[usize] = &[
    880, 3028, 26044, 56601, 31537, 11239, 2482, 376, 86, 25, 4, 1, 0,
];

fn run_corpus(path: &str, tie_break: TieBreak) -> RunResult {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let full_path = Path::new(manifest_dir).join(path);
    let bytes = std::fs::read(&full_path)
        .unwrap_or_else(|e| panic!("не смог прочитать {}: {}", full_path.display(), e));

    let mut store = Store::new();
    store.init_factory();
    let ids = read_and_touch(&mut store, &bytes);
    let seq = Sequence::from_ids(&store, ids);
    let (out, births_per_generation) = merge::run_traced(&mut store, seq, BIRTH_THRESHOLD, tie_break);

    RunResult {
        store,
        births_per_generation,
        final_len: out.len(),
        original_bytes: bytes.len(),
    }
}

fn level_distribution(store: &Store) -> Vec<(u32, usize)> {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for id in 0..store.len() as TokenId {
        *counts.entry(store.level_of(id)).or_default() += 1;
    }
    let mut v: Vec<(u32, usize)> = counts.into_iter().collect();
    v.sort();
    v
}

/// ratio = original_bytes / (final_token_count * bits_per_token / 8),
/// bits_per_token = log2(vocab_size). Та же честная формула, что в
/// docs/axmv2_Отчёт_для_Opus.md (после исправления ошибки единиц ×14).
fn compression_ratio(original_bytes: usize, final_len: usize, vocab_size: usize) -> f64 {
    let bits_per_token = (vocab_size as f64).log2();
    let compressed_bytes = final_len as f64 * bits_per_token / 8.0;
    original_bytes as f64 / compressed_bytes
}

fn top_n_by_count(store: &Store, min_level: u32, n: usize) -> Vec<(Vec<u8>, u32, u64)> {
    let mut items: Vec<(TokenId, u32, u64)> = (0..store.len() as TokenId)
        .filter(|&id| store.level_of(id) >= min_level)
        .map(|id| (id, store.level_of(id), store.count_of(id)))
        .collect();
    items.sort_by(|a, b| b.2.cmp(&a.2));
    items
        .into_iter()
        .take(n)
        .map(|(id, level, count)| (store.bytes_of(id), level, count))
        .collect()
}

/// text -> все (level, count) токенов в этом сторе, рендерящихся в этот
/// текст (может быть несколько на разных уровнях/id — так же, как в
/// out/held_out_symmetric.md python-эталона).
fn render_index(store: &Store) -> HashMap<Vec<u8>, Vec<(u32, u64)>> {
    let mut idx: HashMap<Vec<u8>, Vec<(u32, u64)>> = HashMap::new();
    for id in 0..store.len() as TokenId {
        let level = store.level_of(id);
        if level == 0 {
            continue;
        }
        let bytes = store.bytes_of(id);
        idx.entry(bytes)
            .or_default()
            .push((level, store.count_of(id)));
    }
    idx
}

fn held_out_transfer(native: &Store, other: &Store, label: &str) -> f64 {
    let top = top_n_by_count(native, 2, 100);
    let other_index = render_index(other);
    let mut found = 0usize;
    for (bytes, _level, _count) in &top {
        if other_index.contains_key(bytes) {
            found += 1;
        }
    }
    let pct = 100.0 * found as f64 / top.len() as f64;
    println!(
        "  held-out {}: {}/{} топ-100 (уровень>=2) фрагментов переносятся ({:.1}%)",
        label,
        found,
        top.len(),
        pct
    );
    pct
}

fn report_one(label: &str, tie_break: TieBreak) {
    println!("\n########## {} ##########", label);

    println!("\n=== Стихи 2025.md ===");
    let stihi = run_corpus(STIHI, tie_break);
    println!("прочитано байт: {}", stihi.original_bytes);
    println!("токенов в сторе: {}", stihi.store.len());
    println!("длина последовательности после свёртки: {}", stihi.final_len);
    println!("распределение по уровням: {:?}", level_distribution(&stihi.store));
    println!(
        "коэффициент сжатия: {:.2}x",
        compression_ratio(stihi.original_bytes, stihi.final_len, stihi.store.len())
    );

    println!("\n=== bhagavad_gita.txt ===");
    let gita = run_corpus(GITA, tie_break);
    println!("прочитано байт: {}", gita.original_bytes);
    println!("токенов в сторе: {}", gita.store.len());
    println!("длина последовательности после свёртки: {}", gita.final_len);
    println!("распределение по уровням: {:?}", level_distribution(&gita.store));
    println!(
        "коэффициент сжатия: {:.2}x",
        compression_ratio(gita.original_bytes, gita.final_len, gita.store.len())
    );
    println!("births по поколениям (rust): {:?}", gita.births_per_generation);
    println!("births по итерациям (python): {:?}", PYTHON_GITA_BIRTHS);
    let max_len = gita.births_per_generation.len().max(PYTHON_GITA_BIRTHS.len());
    let mut first_divergence: Option<usize> = None;
    for i in 0..max_len {
        let r = gita.births_per_generation.get(i).copied();
        let p = PYTHON_GITA_BIRTHS.get(i).copied();
        if r != p {
            first_divergence = Some(i);
            break;
        }
    }
    match first_divergence {
        Some(i) => println!(
            "первое расхождение — поколение/итерация #{}: rust={:?}, python={:?}",
            i,
            gita.births_per_generation.get(i),
            PYTHON_GITA_BIRTHS.get(i)
        ),
        None => println!("births по поколениям совпадают побитово с python"),
    }

    println!("\n=== симметричная held-out проверка (топ-100, уровень >= 2) ===");
    held_out_transfer(&stihi.store, &gita.store, "Стихи -> Гита");
    held_out_transfer(&gita.store, &stihi.store, "Гита -> Стихи");
}

fn main() {
    report_one("TieBreak::BirthOrder (принято в шаге 3)", TieBreak::BirthOrder);
    report_one("TieBreak::ContentHash (диагностика: как python)", TieBreak::ContentHash);
}
