//! Шаг 15 (план 3, Правка 5/6): гейты G-B1 и G-B2.
//!
//! G-B1: пермутационный нуль собственного held-out (не «Чехов», который
//! не существует — см. Правку 5 в план 3.md). Протокол: корпус делится
//! на train (первые 90%) и held-out (последние 10%, Колесо их не видело).
//! S-static(held-out) против S-static(переставленный по байтам held-out,
//! фиксированный seed) — зазор обязан быть ≥20%.
//!
//! G-B2: после среза смерти (шаг 12, mark+weight_cut на train) G-B1
//! сохраняется, а длина свёртки held-out ухудшается не более чем на 3%
//! относительно до-среза. Свёртка после среза ограничена множеством
//! `log.focused_set()` (`recognize_focused` — мёртвое недоступно порту).
//!
//! **Диагностика (вне зачёта, по запросу Дениса после первого прогона):**
//! G-B2 прошёл с большим запасом (свёртка held-out стала короче, не
//! просто не хуже) — но held-out (хвост корпуса) стоит рядом с корневым
//! окном (последние K=3 оборота на конец train), а держ-аут — последние
//! 10% train. Улучшение могло быть эффектом БЛИЗОСТИ к корням, не
//! универсальным свойством среза. Проверка: тот же замер на срезе из
//! НАЧАЛА train (тот же размер, что held-out) — максимально далеко от
//! корневого окна по построению (обучение прошло весь train целиком,
//! начало — самое старое). Гейта не меняет, не входит в его вердикт.

use shelf::death::DeathLog;
use shelf::merge::TieBreak;
use shelf::rng::shuffle;
use shelf::surprise::{recognized_length, s_static_focused};
use shelf::wheel::{WheelState, REVOLUTION_CHUNK_BYTES};
use std::path::Path;

const HELD_OUT_FRACTION: f64 = 0.10;
const PERMUTATION_SEED: u64 = 0xB1_5EED;
const G_B1_GAP: f64 = 0.20;
const G_B2_MAX_DEGRADATION: f64 = 0.03;

fn run_on_corpus(corpus_name: &str) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let corpus_path = Path::new(manifest_dir).join(format!("../docs/corpus/{corpus_name}"));
    let bytes = std::fs::read(&corpus_path).expect("corpus");

    let split_at = ((bytes.len() as f64) * (1.0 - HELD_OUT_FRACTION)) as usize;
    let (train, held_out) = bytes.split_at(split_at);

    println!("\n=== {corpus_name} ===");
    println!("train: {} байт, held-out: {} байт", train.len(), held_out.len());

    let mut wheel = WheelState::new();
    for chunk in train.chunks(REVOLUTION_CHUNK_BYTES) {
        wheel.turn(chunk, 2, TieBreak::BirthOrder);
    }
    println!("оборотов на train: {}, токенов в сторе: {}", wheel.revolution, wheel.store.len());

    // --- G-B1 (до среза смерти) ---
    let real = s_static_focused(&wheel.store, held_out, None);
    let mut permuted_bytes = held_out.to_vec();
    shuffle(&mut permuted_bytes, PERMUTATION_SEED);
    let permuted = s_static_focused(&wheel.store, &permuted_bytes, None);
    let gap = if permuted == 0.0 { f64::INFINITY } else { (real - permuted) / permuted };

    println!("\n--- G-B1 (до среза) ---");
    println!("S-static(held-out) = {real:.4}, S-static(переставленный) = {permuted:.4}");
    println!("зазор = {:.1}% (порог >= {:.0}%)", gap * 100.0, G_B1_GAP * 100.0);
    let g_b1_before = gap >= G_B1_GAP;
    println!("G-B1: {}", if g_b1_before { "ПРОЙДЕН" } else { "НЕ ПРОЙДЕН" });

    let length_before_cut = recognized_length(&wheel.store, held_out, None);

    // --- Срез смерти (шаг 12) на train ---
    let mut log = DeathLog::new();
    let mut wheel2 = WheelState::new();
    for chunk in train.chunks(REVOLUTION_CHUNK_BYTES) {
        wheel2.turn(chunk, 2, TieBreak::BirthOrder);
        log.mark(&wheel2);
        log.weight_cut(&wheel2);
    }
    println!("\nпосле среза: {} в фокусе из {} токенов в сторе", log.focused_set().len(), wheel2.store.len());

    // --- G-B1 после среза (сохраняется ли зазор) ---
    let real_after = s_static_focused(&wheel2.store, held_out, Some(log.focused_set()));
    let mut permuted_bytes2 = held_out.to_vec();
    shuffle(&mut permuted_bytes2, PERMUTATION_SEED);
    let permuted_after = s_static_focused(&wheel2.store, &permuted_bytes2, Some(log.focused_set()));
    let gap_after = if permuted_after == 0.0 {
        f64::INFINITY
    } else {
        (real_after - permuted_after) / permuted_after
    };
    println!("\n--- G-B1 (после среза) ---");
    println!("S-static(held-out) = {real_after:.4}, S-static(переставленный) = {permuted_after:.4}");
    println!("зазор = {:.1}%", gap_after * 100.0);
    let g_b1_after = gap_after >= G_B1_GAP;
    println!("G-B1 после среза: {}", if g_b1_after { "СОХРАНЯЕТСЯ" } else { "НЕ СОХРАНЯЕТСЯ" });

    // --- G-B2: длина свёртки held-out не должна ухудшиться больше чем на 3% ---
    let length_after_cut = recognized_length(&wheel2.store, held_out, Some(log.focused_set()));
    let degradation = (length_after_cut as f64 - length_before_cut as f64) / length_before_cut as f64;
    println!("\n--- G-B2 ---");
    println!("длина свёртки held-out: до среза {length_before_cut}, после {length_after_cut}");
    println!("ухудшение = {:.2}% (допуск <= {:.0}%)", degradation * 100.0, G_B2_MAX_DEGRADATION * 100.0);
    let g_b2 = degradation <= G_B2_MAX_DEGRADATION;
    println!("G-B2: {}", if g_b2 { "ПРОЙДЕН" } else { "НЕ ПРОЙДЕН" });

    println!(
        "\nИтог {corpus_name}: G-B1(до)={}, G-B1(после)={}, G-B2={}",
        if g_b1_before { "OK" } else { "FAIL" },
        if g_b1_after { "OK" } else { "FAIL" },
        if g_b2 { "OK" } else { "FAIL" }
    );

    // --- Диагностика вне зачёта: тот же замер на срезе из начала train ---
    let early_slice = &train[..held_out.len().min(train.len())];
    let early_before = recognized_length(&wheel.store, early_slice, None);
    let early_after = recognized_length(&wheel2.store, early_slice, Some(log.focused_set()));
    let early_degradation = (early_after as f64 - early_before as f64) / early_before as f64;
    println!("\n--- Диагностика (вне зачёта): срез из начала train, {} байт ---", early_slice.len());
    println!("длина свёртки: до среза {early_before}, после {early_after}");
    println!(
        "изменение = {:.2}% (для сравнения: на held-out было {:.2}%)",
        early_degradation * 100.0,
        degradation * 100.0
    );
    if early_degradation <= G_B2_MAX_DEGRADATION {
        println!("не хуже допуска G-B2 и на удалённом от корней срезе — эффект похож на универсальный, не только близость к корням.");
    } else {
        println!("хуже допуска G-B2 на удалённом от корней срезе — есть градиент старения: эффект G-B2 был отчасти близостью к корням.");
    }
}

fn main() {
    run_on_corpus("Стихи 2025.md");
    run_on_corpus("bhagavad_gita.txt");
}
