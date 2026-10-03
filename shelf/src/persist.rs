//! Шаг 6: персистентность полки на redb.
//!
//! Персистится ТОЛЬКО тело (level + children, в порядке рождения) — это и
//! есть "неизменная история" по INVARIANTS.md. Счётчики (counts) —
//! восстановимый вид, а не часть истории, и из одного лишь тела не
//! восстанавливаются (нужна была бы отдельная история встреч) — решение
//! их не персистить зафиксировано явно, не тихой экономией места.
//!
//! Append-only: `save` дописывает только токены, рождённые после
//! последнего сохранения (определяется по числу уже записанных строк —
//! id идут подряд от 0, значит длина таблицы == следующий id для записи).
//! `load` проходит лог от начала и воспроизводит `intern()` для каждой
//! записи по порядку — это восстанавливает hash_index/children_arena/
//! tie_hashes/levels ровно тем же путём, что и обычный рост стора.
//!
//! **Инкрементальный save корректен только потому, что стор append-only и
//! тела неизменны** — "дописать хвост по длине уже записанного" опирается
//! на то, что позиции 0..already_persisted никогда не меняются задним
//! числом. Нарушь где-то этот инвариант — и персист-слой первым
//! обнаружит порчу не в абстракции, а в файле на диске.
//!
//! **Границы транзакций:** один вызов `save()` — одна redb-транзакция на
//! весь дозаписываемый хвост целиком (`begin_write()` … `commit()` вокруг
//! всего цикла вставки). Крах посреди `save()` оставляет на диске либо
//! состояние ДО этого вызова (транзакция не закоммитилась), либо
//! состояние ПОСЛЕ (закоммитилась целиком) — redb ACID-гарантия не
//! оставляет промежуточных полу-хвостов ни при каком единичном вызове.
//! Если `save()` вызывается несколько раз за сессию (несколько
//! инкрементальных дозаписей — несколько отдельных транзакций), крах
//! между двумя вызовами восстановит стор по последней успешно
//! закоммиченной дозаписи, потеряв только то, что не успело сохраниться
//! — это осознанная семантика восстановления ("догоняем по последнему
//! коммиту"), а не найденная постфактум случайность.
//!
//! **`load` — не просто десериализация, а верификация лога.** Две реальные
//! проверки, обе в release, не debug-only assert:
//! 1. Ссылки вперёд: запись уровня >=1 не может ссылаться на ребёнка,
//!    который ещё не встретился раньше в логе (append-only — состав
//!    ссылается только на уже рождённое). Ловится ДО вызова `intern()` —
//!    иначе `intern()` запаникует на индексации холодного массива хешей
//!    за границей вместо того, чтобы вернуть `Err` на границе системы.
//! 2. Позиционность: `intern()` при воспроизведении детерминирован — i-й
//!    интернированный состав обязан получить id ровно `i` (append-only,
//!    дедуп по хешу). Если это не так — лог повреждён или переставлен.
//! Оба случая — `LoadError` (`DanglingChild` / `CorruptLog`), не тихое
//! продолжение с рассинхронизированными id и не паника на внешних данных.

use crate::death::{decode_event, encode_event, DeathLog, DEATH_LOG_TABLE};
use crate::store::{Store, TokenId};
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::fmt;
use std::path::Path;

const BODY_TABLE: TableDefinition<u32, (u32, Vec<u32>)> = TableDefinition::new("body");

#[derive(Debug)]
pub enum LoadError {
    Redb(redb::Error),
    /// i-я запись лога воспроизвелась не в id `i` — лог повреждён,
    /// переставлен или писался не через `save()`.
    CorruptLog { expected_id: TokenId, replayed_id: TokenId },
    /// Запись уровня >=1 ссылается на ребёнка, который ещё не появился в
    /// логе к этому моменту — то есть композит был построен из будущего.
    /// Ловится ДО вызова `intern()`: без этой проверки `intern()`
    /// запаникует на индексации холодного массива хешей за границей, а
    /// паника внутри интерна — не тот способ сообщить о повреждённом
    /// внешнем файле (граница системы обязана вернуть Err, не паниковать).
    DanglingChild { record_id: TokenId, child: TokenId },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Redb(e) => write!(f, "redb: {e}"),
            LoadError::CorruptLog { expected_id, replayed_id } => write!(
                f,
                "лог повреждён: запись #{expected_id} воспроизвелась в id {replayed_id}"
            ),
            LoadError::DanglingChild { record_id, child } => write!(
                f,
                "лог повреждён: запись #{record_id} ссылается на несуществующего ребёнка {child}"
            ),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<redb::Error> for LoadError {
    fn from(e: redb::Error) -> Self {
        LoadError::Redb(e)
    }
}

/// Дописать в лог тела токенов, рождённые после последнего save
/// (или все, если лог пуст). Создаёт файл, если его ещё нет (`Database::
/// create` открывает существующий файл, не перезаписывая — безопасно
/// звать повторно на уже существующей базе).
pub fn save(store: &Store, path: &Path) -> Result<(), redb::Error> {
    let db = Database::create(path)?;
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(BODY_TABLE)?;
        let already_persisted = table.len()? as u32;
        for id in already_persisted..store.len() as TokenId {
            let (level, children) = store.get(id);
            table.insert(id, (level, children.to_vec()))?;
        }
    }
    write_txn.commit()?;
    Ok(())
}

/// Восстановить полку проходом по логу — заново вызывая `intern()` в
/// порядке рождения. Счётчики после загрузки — нулевые у всех токенов
/// (не персистились, см. шапку модуля); фабрику (уровень 0) `intern()`
/// восстанавливает как обычные первые 256 записей лога, отдельно звать
/// `init_factory()` не нужно и нельзя — задвоит первые 256 id.
pub fn load(path: &Path) -> Result<Store, LoadError> {
    let db = Database::open(path).map_err(redb::Error::from)?;
    let read_txn = db.begin_read().map_err(redb::Error::from)?;
    let table = read_txn.open_table(BODY_TABLE).map_err(redb::Error::from)?;

    let mut store = Store::new();
    for entry in table.iter().map_err(redb::Error::from)? {
        let (key, value) = entry.map_err(redb::Error::from)?;
        let id = key.value();
        let (level, children) = value.value();

        // Уровень 0: children[0] — сырой байт (0..255), не id ребёнка,
        // проверять не на что. Уровень >=1: каждый ребёнок обязан уже
        // существовать в стopе (append-only — состав может ссылаться
        // только на то, что родилось раньше по логу).
        if level >= 1 {
            if let Some(&bad_child) = children.iter().find(|&&c| c >= store.len() as TokenId) {
                return Err(LoadError::DanglingChild {
                    record_id: id,
                    child: bad_child,
                });
            }
        }

        let replayed = store.intern(level, &children);
        if replayed != id {
            return Err(LoadError::CorruptLog {
                expected_id: id,
                replayed_id: replayed,
            });
        }
    }
    Ok(store)
}

/// Атомарно сохраняет в redb тела токенов Store и события DeathLog в единой транзакции (ACID).
pub fn save_all(store: &Store, death_log: &DeathLog, path: &Path) -> Result<(), redb::Error> {
    let db = Database::create(path)?;
    let write_txn = db.begin_write()?;
    {
        // 1. Сохраняем новые тела токенов
        let mut body_table = write_txn.open_table(BODY_TABLE)?;
        let already_persisted_tokens = body_table.len()? as u32;
        for id in already_persisted_tokens..store.len() as TokenId {
            let (level, children) = store.get(id);
            body_table.insert(id, (level, children.to_vec()))?;
        }

        // 2. Сохраняем новые события лога смерти/фокуса
        let mut death_table = write_txn.open_table(DEATH_LOG_TABLE)?;
        let already_persisted_events = death_table.len()? as u32;
        for (i, &event) in death_log.events().iter().enumerate().skip(already_persisted_events as usize) {
            death_table.insert(i as u32, encode_event(event))?;
        }
    }
    write_txn.commit()?;
    Ok(())
}

/// Восстанавливает Store и DeathLog из redb. Если таблица фокуса отсутствует (старая база),
/// возвращает новый DeathLog.
pub fn load_all(path: &Path) -> Result<(Store, DeathLog), LoadError> {
    let db = Database::open(path).map_err(redb::Error::from)?;
    let read_txn = db.begin_read().map_err(redb::Error::from)?;
    
    // 1. Восстанавливаем Store
    let body_table = read_txn.open_table(BODY_TABLE).map_err(redb::Error::from)?;
    let mut store = Store::new();
    for entry in body_table.iter().map_err(redb::Error::from)? {
        let (key, value) = entry.map_err(redb::Error::from)?;
        let id = key.value();
        let (level, children) = value.value();

        if level >= 1 {
            if let Some(&bad_child) = children.iter().find(|&&c| c >= store.len() as TokenId) {
                return Err(LoadError::DanglingChild {
                    record_id: id,
                    child: bad_child,
                });
            }
        }

        let replayed = store.intern(level, &children);
        if replayed != id {
            return Err(LoadError::CorruptLog {
                expected_id: id,
                replayed_id: replayed,
            });
        }
    }

    // 2. Восстанавливаем DeathLog
    let death_log = match read_txn.open_table(DEATH_LOG_TABLE) {
        Ok(death_table) => {
            let mut events = Vec::new();
            for entry in death_table.iter().map_err(redb::Error::from)? {
                let (_key, value) = entry.map_err(redb::Error::from)?;
                events.push(decode_event(value.value()));
            }
            let focused = DeathLog::replay(&events);
            DeathLog::from_parts(events, focused)
        }
        Err(_) => DeathLog::new(),
    };

    Ok((store, death_log))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("shelf_persist_test_{}_{}.redb", name, std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn round_trip_preserves_body_and_ids() {
        let path = tmp_db_path("round_trip");

        let mut store = Store::new();
        store.init_factory();
        let ab = store.intern(1, &[b'a' as u32, b'b' as u32]);
        let cd = store.intern(1, &[b'c' as u32, b'd' as u32]);
        let abcd = store.intern(2, &[ab, cd]);
        store.touch(abcd); // счётчик НЕ должен пережить round-trip

        save(&store, &path).expect("save");
        let loaded = load(&path).expect("load");

        assert_eq!(loaded.len(), store.len());
        for id in 0..store.len() as TokenId {
            assert_eq!(loaded.get(id), store.get(id), "тело токена id={} разошлось", id);
            assert_eq!(loaded.hash_of(id), store.hash_of(id), "хеш id={} разошёлся", id);
        }
        assert_eq!(loaded.bytes_of(abcd), b"abcd");
        assert_eq!(loaded.count_of(abcd), 0, "счётчики не персистятся — решение зафиксировано явно");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_is_incremental_append_only() {
        let path = tmp_db_path("incremental");

        let mut store = Store::new();
        store.init_factory();
        save(&store, &path).expect("save после фабрики");

        let ab = store.intern(1, &[b'a' as u32, b'b' as u32]);
        store.touch(ab);
        save(&store, &path).expect("save после ещё одного intern");

        let loaded = load(&path).expect("load");
        assert_eq!(loaded.len(), store.len());
        assert_eq!(loaded.bytes_of(ab), b"ab");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn incremental_save_matches_one_shot_save_bit_for_bit() {
        // Ровно тот путь, который "save_is_incremental_append_only" не
        // проверял: save на половине состояния -> дорастить -> save ещё
        // раз -> load. Сравниваем с полкой той же формы, сохранённой
        // ОДНИМ save(). Здесь и только здесь живёт весь класс off-by-one
        // на границе "already_persisted".
        let incremental_path = tmp_db_path("incremental_vs_one_shot__incr");
        let one_shot_path = tmp_db_path("incremental_vs_one_shot__oneshot");

        let mut store = Store::new();
        store.init_factory();
        let ab = store.intern(1, &[b'a' as u32, b'b' as u32]);
        store.touch(ab);
        save(&store, &incremental_path).expect("первая дозапись (после фабрики+ab)");

        let cd = store.intern(1, &[b'c' as u32, b'd' as u32]);
        let abcd = store.intern(2, &[ab, cd]);
        store.touch(cd);
        store.touch(abcd);
        save(&store, &incremental_path).expect("вторая дозапись (после cd+abcd)");

        save(&store, &one_shot_path).expect("контрольный один save на итоговом сторе");

        let loaded_incremental = load(&incremental_path).expect("load инкрементального лога");
        let loaded_one_shot = load(&one_shot_path).expect("load одноразового лога");

        assert_eq!(loaded_incremental.len(), loaded_one_shot.len());
        assert_eq!(loaded_incremental.len(), store.len());
        for id in 0..store.len() as TokenId {
            assert_eq!(
                loaded_incremental.get(id),
                loaded_one_shot.get(id),
                "тело id={} разошлось между инкрементальным и одноразовым save",
                id
            );
            assert_eq!(loaded_incremental.hash_of(id), loaded_one_shot.hash_of(id));
        }

        let _ = std::fs::remove_file(&incremental_path);
        let _ = std::fs::remove_file(&one_shot_path);
    }

    #[test]
    fn load_reconstructs_dedup_index_not_just_bodies() {
        // После загрузки intern() того же состава обязан вернуть тот же id,
        // не завести дубликат — иначе hash_index не восстановился, а
        // просто "похож" на восстановленный.
        let path = tmp_db_path("dedup_after_load");

        let mut store = Store::new();
        store.init_factory();
        let ab = store.intern(1, &[b'a' as u32, b'b' as u32]);
        save(&store, &path).expect("save");

        let mut loaded = load(&path).expect("load");
        let ab_again = loaded.intern(1, &[b'a' as u32, b'b' as u32]);
        assert_eq!(ab_again, ab, "тот же состав после загрузки обязан дать тот же id");
        assert_eq!(loaded.len(), store.len(), "интерн уже существующего состава не должен расти");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_detects_corrupt_log_instead_of_silently_desyncing() {
        // Ломаем лог руками: записываем композит, чей hash при replay
        // дедупнется в УЖЕ существующий id (не в позиционный id записи).
        // Это не "правильно поломанный" файл — просто прямая демонстрация,
        // что load() ловит рассинхронизацию, а не тихо её проглатывает.
        let path = tmp_db_path("corrupt_detection");
        let db = Database::create(&path).expect("create");
        {
            let write_txn = db.begin_write().expect("begin_write");
            {
                let mut table = write_txn.open_table(BODY_TABLE).expect("open_table");
                // id=0 должен быть уровень-0 байт 0 (фабрика) — вместо
                // этого пишем композит уровня 1, ссылающийся на дочерний
                // id=5, которого ещё не существует на момент replay.
                table.insert(0u32, (1u32, vec![5u32])).expect("insert corrupt row");
            }
            write_txn.commit().expect("commit");
        }
        drop(db);

        let result = load(&path);
        assert!(
            matches!(result, Err(_)),
            "повреждённый/несогласованный лог обязан вернуть Err, не тихо собранный стор"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Реальные данные, не игрушечный вход — но маленький корпус (не
    /// bhagavad_gita.txt), чтобы вписаться в обычный `cargo test` в debug-
    /// сборке (~200 мс на "Стихи 2025.md" против ~15 сек на Гите в debug).
    /// Полный корпус — в examples/persist_roundtrip_check.rs, это ручной
    /// гейт перед пачками, не для каждого прогона тестов.
    #[test]
    fn round_trip_on_real_corpus_excerpt() {
        use crate::merge::{self, TieBreak};
        use crate::{read_and_touch, Sequence, BIRTH_THRESHOLD};

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let corpus_path = Path::new(manifest_dir).join("../docs/corpus/Стихи 2025.md");
        let bytes = std::fs::read(&corpus_path).expect("Стихи 2025.md должен быть доступен");

        let mut store = Store::new();
        store.init_factory();
        let ids = read_and_touch(&mut store, &bytes);
        let seq = Sequence::from_ids(&store, ids);
        merge::run(&mut store, seq, BIRTH_THRESHOLD, TieBreak::BirthOrder);

        let path = tmp_db_path("real_corpus_excerpt");
        save(&store, &path).expect("save");
        let loaded = load(&path).expect("load");

        assert_eq!(loaded.len(), store.len());
        for id in 0..store.len() as TokenId {
            assert_eq!(loaded.get(id), store.get(id));
            assert_eq!(loaded.hash_of(id), store.hash_of(id));
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_save_all_and_load_all_round_trip() {
        use crate::wheel::WheelState;
        use crate::merge::TieBreak;

        let path = tmp_db_path("save_all_round_trip");
        let mut wheel = WheelState::new();
        wheel.turn(b"hello world of axmg", 2, TieBreak::BirthOrder);

        let mut death_log = DeathLog::new();
        death_log.mark(&wheel);
        death_log.weight_cut(&wheel);

        save_all(&wheel.store, &death_log, &path).expect("save_all");

        let (loaded_store, loaded_log) = load_all(&path).expect("load_all");

        assert_eq!(loaded_store.len(), wheel.store.len());
        assert_eq!(loaded_log.events().len(), death_log.events().len());
        assert_eq!(loaded_log.focused_set(), death_log.focused_set());

        let _ = std::fs::remove_file(&path);
    }
}
