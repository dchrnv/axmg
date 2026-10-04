use axmg::merge::TieBreak;
use axmg::{merge, persist, read_and_touch, Sequence, Store, TokenId, BIRTH_THRESHOLD};
use std::path::Path;

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let corpus_path = Path::new(manifest_dir).join("../docs/corpus/bhagavad_gita.txt");
    let bytes = std::fs::read(&corpus_path).expect("bhagavad_gita.txt");

    let mut store = Store::new();
    store.init_factory();
    let ids = read_and_touch(&mut store, &bytes);
    let seq = Sequence::from_ids(&store, ids);
    merge::run(&mut store, seq, BIRTH_THRESHOLD, TieBreak::ContentHash);
    println!("токенов: {}", store.len());

    // "Логическая" полезная нагрузка: level (varint) + children (raw u32 each).
    let mut logical_bytes: u64 = 0;
    let mut delta_varint_bytes: u64 = 0;
    for id in 0..store.len() as TokenId {
        let (_level, children) = store.get(id);
        logical_bytes += 1; // level помещается в 1 байт (уровни < 256)
        logical_bytes += (children.len() * 4) as u64; // текущее кодирование: u32 на ребёнка

        // Гипотеза дельта-кодирования: ребёнок всегда < id родителя
        // (append-only), значит delta = id - child всегда > 0. Кодируем
        // varint (LEB128) от delta вместо сырых 4 байт.
        for &c in children {
            let delta = id - c;
            delta_varint_bytes += varint_len(delta as u64);
        }
        delta_varint_bytes += 1; // level
    }
    println!("логическая нагрузка (текущее кодирование, level+children как есть): {} байт ({:.1} байт/токен)",
        logical_bytes, logical_bytes as f64 / store.len() as f64);
    println!("логическая нагрузка (varint-дельта детей): {} байт ({:.1} байт/токен)",
        delta_varint_bytes, delta_varint_bytes as f64 / store.len() as f64);

    let db_path = std::env::temp_dir().join("shelf_storage_breakdown.redb");
    let _ = std::fs::remove_file(&db_path);
    persist::save(&store, &db_path).expect("save");
    let file_size = std::fs::metadata(&db_path).unwrap().len();
    println!("файл redb: {} байт ({:.1} байт/токен)", file_size, file_size as f64 / store.len() as f64);
    println!("во сколько раз файл больше логической нагрузки (текущее кодирование): {:.1}x",
        file_size as f64 / logical_bytes as f64);

    // Контроль: что даст generic-компрессия (zstd/gzip-подобная) поверх
    // сырого тела лога, без всякого редактирования формата — верхняя
    // граница того, что можно выжать вообще.
    let mut raw_body_bytes: Vec<u8> = Vec::new();
    for id in 0..store.len() as TokenId {
        let (level, children) = store.get(id);
        raw_body_bytes.push(level as u8);
        raw_body_bytes.push(children.len() as u8);
        for &c in children {
            raw_body_bytes.extend_from_slice(&c.to_le_bytes());
        }
    }
    println!("сырой тела-лог без redb (наивная упаковка): {} байт", raw_body_bytes.len());

    let _ = std::fs::remove_file(&db_path);
}

fn varint_len(mut v: u64) -> u64 {
    let mut n = 1u64;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}
