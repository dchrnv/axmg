//! Шаг 8 (A1, план 3): стор связей перевёрнутой библиотеки.
//!
//! Связь = (token, context_token) — не рекурсивна (в отличие от тела
//! прямой полки: связь не ссылается на другие связи), поэтому
//! дедупликация напрямую по паре, без content-хеширования — пара сама
//! себе ключ.
//!
//! Тело (какие связи существуют) отделено от счётчика (сколько раз связь
//! встретилась) — та же дисциплина, что у Store (INVARIANTS.md:
//! "разделение тела и счётчика"). Счётчик здесь не строится — это работа
//! шага 9 (cooc.rs, COO/CSR). `intern_all` намеренно отбрасывает вес из
//! потока экстрактора: тело фиксирует факт существования связи, не её
//! частоту.
//!
//! Связь односторонняя (план 3, шаг 8): перевёрнутая библиотека ссылается
//! на `TokenId` прямой полки, прямая полка о перевёрнутой не знает —
//! `InvertedStore` не хранит и не требует `&Store` для своей идентичности,
//! только `TokenId`-значения.
//!
//! Персист — тот же redb-паттерн, что у шага 6 (`persist.rs`): append-only
//! лог рождений связей, `load` воспроизводит `intern_link()` по порядку и
//! проверяет позиционность (тот же класс проверки, что `LoadError::
//! CorruptLog` у прямой полки).

use crate::store::TokenId;
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

pub type LinkId = u32;

pub struct InvertedStore {
    links: Vec<(TokenId, TokenId)>,
    link_index: HashMap<(TokenId, TokenId), LinkId>,
    birth_log: Vec<LinkId>,
}

impl InvertedStore {
    pub fn new() -> Self {
        InvertedStore {
            links: Vec::new(),
            link_index: HashMap::new(),
            birth_log: Vec::new(),
        }
    }

    /// Завести связь, если её ещё не было; вернуть её `LinkId` в любом
    /// случае (дедупликация по паре, как `intern()` у `Store`).
    pub fn intern_link(&mut self, token: TokenId, context: TokenId) -> LinkId {
        let key = (token, context);
        if let Some(&id) = self.link_index.get(&key) {
            return id;
        }
        let id = self.links.len() as LinkId;
        self.links.push(key);
        self.birth_log.push(id);
        self.link_index.insert(key, id);
        id
    }

    /// Завести много связей разом из потока событий экстрактора. Вес
    /// потока здесь отбрасывается — см. шапку модуля.
    pub fn intern_all(&mut self, events: &[(TokenId, TokenId, u64)]) {
        for &(token, context, _weight) in events {
            self.intern_link(token, context);
        }
    }

    pub fn get(&self, id: LinkId) -> (TokenId, TokenId) {
        self.links[id as usize]
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    pub fn birth_log(&self) -> &[LinkId] {
        &self.birth_log
    }
}

impl Default for InvertedStore {
    fn default() -> Self {
        Self::new()
    }
}

const LINK_TABLE: TableDefinition<u32, (u32, u32)> = TableDefinition::new("links");

#[derive(Debug)]
pub enum InvertedLoadError {
    Redb(redb::Error),
    /// i-я запись лога воспроизвелась не в LinkId `i` — лог повреждён
    /// или переставлен (тот же класс проверки, что `persist::LoadError::
    /// CorruptLog`).
    CorruptLog { expected_id: LinkId, replayed_id: LinkId },
}

impl fmt::Display for InvertedLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvertedLoadError::Redb(e) => write!(f, "redb: {e}"),
            InvertedLoadError::CorruptLog { expected_id, replayed_id } => write!(
                f,
                "лог связей повреждён: запись #{expected_id} воспроизвелась в id {replayed_id}"
            ),
        }
    }
}

impl std::error::Error for InvertedLoadError {}

impl From<redb::Error> for InvertedLoadError {
    fn from(e: redb::Error) -> Self {
        InvertedLoadError::Redb(e)
    }
}

/// Дописать в лог связи, рождённые после последнего save (или все, если
/// лог пуст) — тот же инкрементальный паттерн, что `persist::save`.
pub fn save(store: &InvertedStore, path: &Path) -> Result<(), redb::Error> {
    let db = Database::create(path)?;
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(LINK_TABLE)?;
        let already_persisted = table.len()? as u32;
        for id in already_persisted..store.len() as LinkId {
            let (token, context) = store.get(id);
            table.insert(id, (token, context))?;
        }
    }
    write_txn.commit()?;
    Ok(())
}

/// Восстановить стор связей проходом по логу, воспроизводя `intern_link`
/// в порядке рождения. Позиционность проверяется так же, как у прямой
/// полки: i-я запись обязана воспроизвестись в LinkId `i`.
pub fn load(path: &Path) -> Result<InvertedStore, InvertedLoadError> {
    let db = Database::open(path).map_err(redb::Error::from)?;
    let read_txn = db.begin_read().map_err(redb::Error::from)?;
    let table = read_txn.open_table(LINK_TABLE).map_err(redb::Error::from)?;

    let mut store = InvertedStore::new();
    for entry in table.iter().map_err(redb::Error::from)? {
        let (key, value) = entry.map_err(redb::Error::from)?;
        let id = key.value();
        let (token, context) = value.value();
        let replayed = store.intern_link(token, context);
        if replayed != id {
            return Err(InvertedLoadError::CorruptLog {
                expected_id: id,
                replayed_id: replayed,
            });
        }
    }
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ContextExtractor, Neighbor, Parent};
    use crate::merge::{self, Sequence, TieBreak};
    use crate::{read_and_touch, Store, BIRTH_THRESHOLD};

    fn tmp_db_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("shelf_inverted_test_{}_{}.redb", name, std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn intern_link_dedupes_by_pair_and_is_directed() {
        let mut store = InvertedStore::new();
        let ab1 = store.intern_link(1, 2);
        let ab2 = store.intern_link(1, 2);
        let ba = store.intern_link(2, 1);
        assert_eq!(ab1, ab2, "та же пара -> тот же LinkId");
        assert_ne!(ab1, ba, "направление участвует в идентичности связи");
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn intern_all_drops_weight_keeps_distinct_links() {
        let mut store = InvertedStore::new();
        store.intern_all(&[(1, 2, 5), (1, 2, 3), (2, 3, 1)]);
        assert_eq!(store.len(), 2, "вес не создаёт новых связей, только повторяет существующую");
    }

    #[test]
    fn round_trip_preserves_links_and_ids() {
        let path = tmp_db_path("round_trip");
        let mut store = InvertedStore::new();
        store.intern_link(10, 20);
        store.intern_link(20, 30);
        store.intern_link(10, 20); // повтор, не должен родить новую запись

        save(&store, &path).expect("save");
        let loaded = load(&path).expect("load");

        assert_eq!(loaded.len(), store.len());
        for id in 0..store.len() as LinkId {
            assert_eq!(loaded.get(id), store.get(id), "связь id={} разошлась", id);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_is_incremental_append_only() {
        let path = tmp_db_path("incremental");
        let mut store = InvertedStore::new();
        store.intern_link(1, 2);
        save(&store, &path).expect("save 1");

        store.intern_link(2, 3);
        save(&store, &path).expect("save 2");

        let loaded = load(&path).expect("load");
        assert_eq!(loaded.len(), store.len());
        assert_eq!(loaded.get(0), (1, 2));
        assert_eq!(loaded.get(1), (2, 3));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_detects_corrupt_log() {
        let path = tmp_db_path("corrupt");
        let db = Database::create(&path).expect("create");
        {
            let write_txn = db.begin_write().expect("begin_write");
            {
                let mut table = write_txn.open_table(LINK_TABLE).expect("open_table");
                // Пропускаем позицию 0, пишем сразу под ключом 1 — при
                // replay первая же запись (token=5, context=6) получит
                // LinkId 0, а не 1: рассинхронизация.
                table.insert(1u32, (5u32, 6u32)).expect("insert");
            }
            write_txn.commit().expect("commit");
        }
        drop(db);

        let result = load(&path);
        assert!(matches!(result, Err(InvertedLoadError::CorruptLog { .. })));
        let _ = std::fs::remove_file(&path);
    }

    /// Стоп-условие шага 8 (план 3): roundtrip персиста связей на
    /// реальном корпусе, 0 расхождений. Маленький корпус ("Стихи
    /// 2025.md") — быстро в debug-сборке, см. обоснование в persist.rs.
    #[test]
    fn round_trip_on_real_corpus_links() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let corpus_path = Path::new(manifest_dir).join("../docs/corpus/Стихи 2025.md");
        let bytes = std::fs::read(&corpus_path).expect("Стихи 2025.md должен быть доступен");

        let mut direct = Store::new();
        direct.init_factory();
        let ids = read_and_touch(&mut direct, &bytes);
        let seq = Sequence::from_ids(&direct, ids);
        let out = merge::run(&mut direct, seq, BIRTH_THRESHOLD, TieBreak::BirthOrder);

        let mut inverted = InvertedStore::new();
        inverted.intern_all(&Neighbor.extract(&direct, &out.ids));
        inverted.intern_all(&Parent.extract(&direct, &out.ids));
        assert!(!inverted.is_empty(), "на реальном корпусе связи обязаны появиться");

        let path = tmp_db_path("real_corpus_links");
        save(&inverted, &path).expect("save");
        let loaded = load(&path).expect("load");

        assert_eq!(loaded.len(), inverted.len());
        let mut mismatches = 0;
        for id in 0..inverted.len() as LinkId {
            if loaded.get(id) != inverted.get(id) {
                mismatches += 1;
            }
        }
        assert_eq!(mismatches, 0, "расхождений связей после round-trip: {mismatches}");

        let _ = std::fs::remove_file(&path);
    }
}
