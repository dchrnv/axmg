//! A4 (план 3, Правка 2): гейт G-A1′.
//!
//! G-A1 (Правка 1, отношение ДОЛЕЙ) оказался дефектным по конструкции:
//! доля ограничена единицей сверху, при доле на переставленном корпусе
//! 0.286 потолок отношения — 1/0.286 ≈ 3.5×, порог ×10 был математически
//! недостижим при любом корпусе. Обнаружено стоп-протоколом (гейт не
//! прошёл — цифры разобраны, а не подогнаны), не результат эксперимента.
//!
//! G-A1′ (Правка 2): отношение АБСОЛЮТНЫХ чисел валидных связей —
//! `валидных(реальный) / валидных(переставленный) ≥ ×10`, без потолка.
//! Полы прежние (валидных на реальном ≥1000, оценённых на переставленном
//! ≥200).
//!
//! Фальсифицируемость: числа bhagavad_gita.txt по этой метрике уже
//! видены (в отчёте ниже — как exploratory, вне зачёта, не как вердикт).
//! Зачётный прогон — «Стихи 2025.md», корпус, не подсматривавшийся под
//! эту метрику; порог ×10 зафиксирован до прогона на нём.

use axmg::context::Window;
use axmg::merge::TieBreak;
use axmg::ppmi::{self, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD};
use axmg::second_shelf::accumulate_windowed_per_generation;
use axmg::{read_and_touch, Sequence, Store, BIRTH_THRESHOLD};
use std::path::Path;

const PERMUTATION_SEED: u64 = 0xA4_5EED;

const G_A1_PRIME_RATIO: f64 = 10.0;
const G_A1_PRIME_MIN_VALID_REAL: usize = 1000;
const G_A1_PRIME_MIN_EVALUATED_PERMUTED: usize = 200;

struct RunResult {
    valid_real: usize,
    evaluated_real: usize,
    valid_permuted: usize,
    evaluated_permuted: usize,
}

fn run_on_corpus(path: &Path) -> RunResult {
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

    let (valid_real, evaluated_real) =
        ppmi::valid_fraction(&csr_real, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD);
    let (valid_permuted, evaluated_permuted) =
        ppmi::valid_fraction(&csr_permuted, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD);

    RunResult {
        valid_real,
        evaluated_real,
        valid_permuted,
        evaluated_permuted,
    }
}

fn report(label: &str, r: &RunResult, apply_verdict: bool) -> bool {
    let ratio = if r.valid_permuted == 0 {
        if r.valid_real == 0 {
            1.0
        } else {
            f64::INFINITY
        }
    } else {
        r.valid_real as f64 / r.valid_permuted as f64
    };

    println!("\n=== {label} ===");
    println!("валидных реальный: {} (оценено {})", r.valid_real, r.evaluated_real);
    println!("валидных переставленный: {} (оценено {})", r.valid_permuted, r.evaluated_permuted);
    println!(
        "G-A1′ отношение (валидных реальный / валидных переставленный): {}",
        if ratio.is_infinite() { "∞".to_string() } else { format!("{ratio:.2}") }
    );

    let cond_ratio = ratio >= G_A1_PRIME_RATIO;
    let cond_min_real = r.valid_real >= G_A1_PRIME_MIN_VALID_REAL;
    let cond_min_denominator = r.evaluated_permuted >= G_A1_PRIME_MIN_EVALUATED_PERMUTED;
    let passed = cond_ratio && cond_min_real && cond_min_denominator;

    println!(
        "  отношение >= {G_A1_PRIME_RATIO}x: {cond_ratio}; валидных на реальном >= {G_A1_PRIME_MIN_VALID_REAL}: {cond_min_real} ({}); оценённых на переставленном >= {G_A1_PRIME_MIN_EVALUATED_PERMUTED}: {cond_min_denominator} ({})",
        r.valid_real, r.evaluated_permuted
    );

    if apply_verdict {
        println!("  G-A1′ (ЗАЧЁТНЫЙ ВЕРДИКТ): {}", if passed { "ПРОЙДЕН" } else { "НЕ ПРОЙДЕН" });
    } else {
        println!("  (exploratory — числа уже видены по этой метрике, вне зачёта, вердикт не выносится)");
    }

    passed
}

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    let gita_path = Path::new(manifest_dir).join("../docs/corpus/bhagavad_gita.txt");
    let gita = run_on_corpus(&gita_path);
    report("bhagavad_gita.txt (exploratory, вне зачёта — план 3, Правка 2)", &gita, false);

    let stihi_path = Path::new(manifest_dir).join("../docs/corpus/Стихи 2025.md");
    let stihi = run_on_corpus(&stihi_path);
    let passed = report("Стихи 2025.md (зачётный прогон G-A1′)", &stihi, true);

    if passed {
        println!("\nG-A1′ пройден на незнакомом корпусе — пачка A может двигаться к A5 (добыча).");
    } else {
        println!("\nG-A1′ не пройден на незнакомом корпусе — стоп, цифры выше. Это содержательный результат про зависимость со-встречаемости от жанра/плотности повторов, не бухгалтерская неудача (план 3, Правка 2).");
    }
}
