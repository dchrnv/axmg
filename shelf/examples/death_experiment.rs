//! Шаг 12 (план 3): прогон Колеса + mark на реальном корпусе —
//! стоп-условие: replay лога восстанавливает то же живое множество
//! побитово. "Любой прогон чтения через порт видит только живое" не
//! проверяется буквально (порт — шаг 14, не построен), проверяется его
//! содержательная часть: множество достижимых токенов детерминировано
//! и воспроизводимо из лога, не из побочного состояния.

use shelf::death::{self, DeathLog, ROOT_WINDOW_REVOLUTIONS};
use shelf::merge::TieBreak;
use shelf::wheel::{WheelState, REVOLUTION_CHUNK_BYTES};
use std::path::Path;
use std::time::Instant;

fn run_and_report(corpus_name: &str) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let corpus_path = Path::new(manifest_dir).join(format!("../docs/corpus/{corpus_name}"));
    let bytes = std::fs::read(&corpus_path).expect("corpus");

    println!("\n=== {corpus_name} ({} байт) ===", bytes.len());

    let mut wheel = WheelState::new();
    let mut log = DeathLog::new();

    let t0 = Instant::now();
    for chunk in bytes.chunks(REVOLUTION_CHUNK_BYTES) {
        wheel.turn(chunk, 2, TieBreak::BirthOrder);
        let outcome = log.mark(&wheel);
        let cut = log.weight_cut(&wheel);
        if wheel.revolution <= 3 || wheel.revolution % 10 == 0 {
            println!(
                "  оборот {}: +{} фокус, -{} расфокус, {} без изменений; срез по весу: {}/{} пограничных",
                wheel.revolution, outcome.newly_focused, outcome.newly_unfocused, outcome.unchanged,
                cut.cut_count, cut.borderline_count
            );
        }
    }
    println!("оборотов: {}, время: {:?}", wheel.revolution, t0.elapsed());
    println!("токенов в сторе: {}", wheel.store.len());
    println!("в фокусе на конец прогона: {}", log.focused_set().len());
    println!("событий в логе фокуса: {}", log.events().len());

    let roots = death::root_set(&wheel);
    println!("корней (K={} оборота + фабрика): {}", ROOT_WINDOW_REVOLUTIONS, roots.len());

    // Стоп-условие: replay из лога -> то же множество побитово.
    let replayed = DeathLog::replay(log.events());
    assert_eq!(&replayed, log.focused_set(), "replay обязан совпасть с накопленным состоянием побитово");
    println!("replay: {} токенов, совпадает с накопленным состоянием — OK", replayed.len());

    // Персист-раунд-трип на реальных данных.
    let db_path = std::env::temp_dir().join(format!("shelf_death_{corpus_name}.redb"));
    let _ = std::fs::remove_file(&db_path);
    death::save(&log, &db_path).expect("save");
    let loaded = death::load(&db_path).expect("load");
    assert_eq!(loaded.events(), log.events());
    assert_eq!(loaded.focused_set(), log.focused_set());
    println!("persist round-trip: {} событий, совпадает побитово — OK", loaded.events().len());
    let _ = std::fs::remove_file(&db_path);
}

fn main() {
    run_and_report("Стихи 2025.md");
    run_and_report("bhagavad_gita.txt");
}
