//! Ручной гейт перед пачками (не для cargo test — ~15 сек в debug на
//! полном корпусе, ~3 сек в release; дешёвый вариант на маленьком
//! корпусе живёт в persist::tests::round_trip_on_real_corpus_excerpt).
//!
//! Два прогона:
//! 1. Одноразовый save() на полностью построенной полке.
//! 2. Инкрементальный: save() после чтения байт (checkpoint сессии 1),
//!    потом save() ещё раз после каскада (checkpoint сессии 2) — именно
//!    тот путь, где живёт весь класс off-by-one на границе
//!    "already_persisted". Сверяются побитово.

use axmg::merge::TieBreak;
use axmg::{merge, persist, read_and_touch, Sequence, Store, TokenId, BIRTH_THRESHOLD};
use std::path::Path;
use std::time::Instant;

fn corpus_bytes() -> Vec<u8> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let corpus_path = Path::new(manifest_dir).join("../docs/corpus/bhagavad_gita.txt");
    std::fs::read(&corpus_path).expect("bhagavad_gita.txt")
}

fn compare_bit_for_bit(a: &Store, b: &Store, label: &str) {
    assert_eq!(a.len(), b.len(), "{label}: разная длина словаря");
    let mut mismatches = 0;
    for id in 0..a.len() as TokenId {
        if a.get(id) != b.get(id) || a.hash_of(id) != b.hash_of(id) {
            mismatches += 1;
        }
    }
    println!("{label}: расхождений тел токенов: {mismatches}");
    assert_eq!(mismatches, 0, "{label}: тела разошлись");
}

fn main() {
    let bytes = corpus_bytes();

    // --- Прогон 1: одноразовый save на полностью построенной полке ---
    let mut one_shot_store = Store::new();
    one_shot_store.init_factory();
    let ids = read_and_touch(&mut one_shot_store, &bytes);
    let seq = Sequence::from_ids(&one_shot_store, ids);
    merge::run(&mut one_shot_store, seq, BIRTH_THRESHOLD, TieBreak::ContentHash);
    println!("построено токенов: {}", one_shot_store.len());

    let one_shot_path = std::env::temp_dir().join("shelf_gita_one_shot.redb");
    let _ = std::fs::remove_file(&one_shot_path);
    let t0 = Instant::now();
    persist::save(&one_shot_store, &one_shot_path).expect("save (one-shot)");
    println!(
        "save (one-shot): {:?}, файл: {} байт",
        t0.elapsed(),
        std::fs::metadata(&one_shot_path).unwrap().len()
    );
    let t1 = Instant::now();
    let loaded_one_shot = persist::load(&one_shot_path).expect("load (one-shot)");
    println!("load (one-shot): {:?}", t1.elapsed());
    compare_bit_for_bit(&one_shot_store, &loaded_one_shot, "one-shot: store vs loaded");

    // --- Прогон 2: два save() за сессию — checkpoint после чтения байт,
    // checkpoint после каскада. Инкрементальный путь ("already_persisted"). ---
    let incremental_path = std::env::temp_dir().join("shelf_gita_incremental.redb");
    let _ = std::fs::remove_file(&incremental_path);

    let mut incremental_store = Store::new();
    incremental_store.init_factory();
    let ids = read_and_touch(&mut incremental_store, &bytes);
    let t2 = Instant::now();
    persist::save(&incremental_store, &incremental_path).expect("save после чтения байт");
    println!(
        "save (checkpoint 1, после {} токенов уровня 0): {:?}",
        incremental_store.len(),
        t2.elapsed()
    );

    let seq = Sequence::from_ids(&incremental_store, ids);
    merge::run(&mut incremental_store, seq, BIRTH_THRESHOLD, TieBreak::ContentHash);
    let t3 = Instant::now();
    persist::save(&incremental_store, &incremental_path).expect("save после каскада");
    println!(
        "save (checkpoint 2, после каскада, {} токенов): {:?}",
        incremental_store.len(),
        t3.elapsed()
    );

    let loaded_incremental = persist::load(&incremental_path).expect("load (incremental)");
    compare_bit_for_bit(&incremental_store, &loaded_incremental, "incremental: store vs loaded");
    compare_bit_for_bit(
        &loaded_one_shot,
        &loaded_incremental,
        "loaded one-shot vs loaded incremental (решающая сверка)",
    );

    let _ = std::fs::remove_file(&one_shot_path);
    let _ = std::fs::remove_file(&incremental_path);
    println!("round-trip на реальном корпусе (одноразовый и инкрементальный) — OK");
}
