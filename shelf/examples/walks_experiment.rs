//! A5 (план 3, Правка 4 — окончательная конструкция): гейт G-A2″ —
//! walk-PPMI с эмпирическим вычитанием структурного фона.
//!
//! Зачёт — на bhagavad_gita.txt (Р-А5-3, не менялось Правкой 4). «Стихи»
//! — вторым номером, вне зачёта.
//!
//! Метрика: |кандидаты(реальный)| / |кандидаты(переставленный)| >= ×5,
//! реальный >= 100; fallback при переставленном < 20: реальный >= 100.
//! Числа не менялись в третий раз — консервативны относительно всего,
//! что видели слабые метрики (1.09, 2.32, 3.20).
//!
//! ТЕРМИНАЛЬНО: это последняя форма гейта. Провал — принятый
//! отрицательный результат (шаг 10 сдаётся инфраструктурой без
//! майнингового заявления, пачка A закрывается на G-A1′+G-A3), не повод
//! для новой правки.

use shelf::context::Window;
use shelf::merge::TieBreak;
use shelf::ppmi::{DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD};
use shelf::second_shelf::accumulate_windowed_per_generation;
use shelf::walks::mine_via_walk_ppmi;
use shelf::{read_and_touch, Sequence, Store, BIRTH_THRESHOLD};
use std::path::Path;

const WALK_SEED_REAL: u64 = 0xA5_5EED;
const WALK_SEED_PERMUTED: u64 = 0xA5_5EED + 1;
const WALK_SEED_STRUCTURAL: u64 = 0xA5_5EED + 2;
const PERMUTATION_SEED: u64 = 0xA4_5EED;

const G_A2_RATIO: f64 = 5.0;
const G_A2_MIN_REAL: usize = 100;
const G_A2_FALLBACK_MAX_PERMUTED: usize = 20;

fn run_on_corpus(path: &Path, label: &str, apply_verdict: bool) -> (usize, usize, bool) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("не смог прочитать {}: {}", path.display(), e));

    let mut store = Store::new();
    store.init_factory();
    let ids = read_and_touch(&mut store, &bytes);
    let seq = Sequence::from_ids(&store, ids);

    let window = Window::range(2, 5);
    let (_out, coo) = accumulate_windowed_per_generation(
        &mut store,
        seq,
        BIRTH_THRESHOLD,
        TieBreak::BirthOrder,
        &window,
        2,
        PERMUTATION_SEED,
    );

    let csr_real = coo.real.bake_to_csr();
    let csr_permuted = coo.permuted.bake_to_csr();

    let outcome = mine_via_walk_ppmi(
        &store,
        &csr_real,
        &csr_permuted,
        DEFAULT_FREQUENCY_FLOOR,
        DEFAULT_PPMI_THRESHOLD,
        WALK_SEED_REAL,
        WALK_SEED_PERMUTED,
        WALK_SEED_STRUCTURAL,
    );

    println!("\n=== {label} ===");
    println!(
        "walk-PPMI-валидных: реальный={}, переставленный={}, структурный фон B={}",
        outcome.walk_valid_real,
        outcome.walk_valid_permuted,
        outcome.background.len()
    );
    println!(
        "кандидаты (после вычитания фона/полки/родни): реальный={}, переставленный={}",
        outcome.candidates_real.len(),
        outcome.candidates_permuted.len()
    );

    let real_n = outcome.candidates_real.len();
    let permuted_n = outcome.candidates_permuted.len();

    let passed = if permuted_n < G_A2_FALLBACK_MAX_PERMUTED {
        println!("знаменатель {permuted_n} < {G_A2_FALLBACK_MAX_PERMUTED} -> fallback: реальный >= {G_A2_MIN_REAL}");
        real_n >= G_A2_MIN_REAL
    } else {
        let ratio = real_n as f64 / permuted_n as f64;
        println!("отношение: {real_n}/{permuted_n} = {ratio:.2} (порог >= {G_A2_RATIO})");
        ratio >= G_A2_RATIO && real_n >= G_A2_MIN_REAL
    };

    if apply_verdict {
        println!("G-A2″ (ЗАЧЁТНЫЙ ВЕРДИКТ, ТЕРМИНАЛЬНЫЙ): {}", if passed { "ПРОЙДЕН" } else { "НЕ ПРОЙДЕН" });
    } else {
        println!("(вне зачёта — кросс-жанровая проверка)");
    }

    if passed && !outcome.candidates_real.is_empty() {
        println!("топ-20 кандидатов (первые по нормализованному (min,max) TokenId — не ранжированы весом, это множество, не список с весами):");
        for &(a, b) in outcome.candidates_real.iter().take(20) {
            let ta = store.bytes_of(a);
            let tb = store.bytes_of(b);
            println!("  ({:?} ~ {:?})", String::from_utf8_lossy(&ta), String::from_utf8_lossy(&tb));
        }
    }

    (real_n, permuted_n, passed)
}

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    let gita_path = Path::new(manifest_dir).join("../docs/corpus/bhagavad_gita.txt");
    let (real_n, permuted_n, passed) = run_on_corpus(&gita_path, "bhagavad_gita.txt (зачётный прогон G-A2″)", true);

    let stihi_path = Path::new(manifest_dir).join("../docs/corpus/Стихи 2025.md");
    run_on_corpus(&stihi_path, "Стихи 2025.md (вне зачёта, кросс-жанровая проверка)", false);

    println!(
        "\nИтог (терминальный): G-A2″ на Гите — {} ({real_n}/{permuted_n})",
        if passed { "ПРОЙДЕН" } else { "НЕ ПРОЙДЕН" }
    );
    if !passed {
        println!("Отрицательный результат принят: блуждания на данном корпусе/конструкции графа не добывают латентного сигнала сверх структуры и прямой со-встречаемости. Шаг 10 сдаётся инфраструктурой без майнингового заявления. Пачка A закрывается на G-A1′+G-A3.");
    }
}
