//! Шаг 1: интернирование id через u32-таблицу.
//!
//! Хеш (SHA-256, 32 байта) хранится один раз в холодном массиве `hashes`,
//! индексированном u32. Везде остальном (children, sequence, счётчики)
//! работаем через `TokenId = u32`, не через сырые байты хеша.
//!
//! Хеш композита считается от хешей непосредственных детей (см.
//! INVARIANTS.md — "id считается от id непосредственных детей, а не от
//! полного байтового раскрытия поддерева"), а не от u32-индекса ребёнка:
//! индекс — локальная метка рождения в конкретном сторе, хеш — то, что
//! делает id производным от состава.
//!
//! Шаг 5: children — SoA-арена, не `Vec<Vec<TokenId>>` (россыпь отдельных
//! heap-аллокаций, одна на токен). Все дети всех токенов лежат подряд в
//! одном плоском `children_arena`; метаданные токена (`children_start`,
//! `children_len`) — просто смещение и длина среза. Арена только растёт —
//! `extend_from_slice` в конец, ничего не переставляется — поэтому
//! смещения, once assigned, никогда не двигаются: append-only гарантирует
//! стабильность среза на всё время жизни стора (важно для шага 6 —
//! персистентность может ссылаться на смещения не опасаясь, что они
//! "уедут").
//!
//! Шаг 7 (осознание, не реализация): полка — это Merkle DAG, изоморфный
//! git-объектнику. `id = hash(level, hash(child_1), hash(child_2), ...)` —
//! ровно та же конструкция, что у git-дерева/blob: узел адресуется хешем
//! своего содержимого, содержимое — это ссылки (хеши) на другие узлы,
//! DAG кладётся один раз и никогда не мутирует (append-only = git-объекты
//! неизменны после коммита). intern() = `git hash-object`, birth_log =
//! порядок появления объектов, redb-тело = буквально то, что в git лежит
//! в `.git/objects/`. Из этой изоморфии план предлагает позаимствовать
//! packfiles (много объектов в одном сжатом блобе — компрессия по
//! батчу) и дельта-кодирование детей (компактная запись ссылки на
//! соседний по времени объект). Измерено на bhagavad_gita.txt
//! (`examples/storage_breakdown.rs`): дельта-кодирование детей срезало
//! бы логическую нагрузку с 9.0 до 6.7 байт/токен, но файл redb (63.6
//! байт/токен) больше логической нагрузки в 7.1 раза — вес сидит в
//! B-tree-структуре redb (ключ+служебные страницы на запись), а не в
//! формате записи. Дельта-кодирование в текущем виде не решает
//! измеримую проблему; packfile-подход (батчевая компрессия, redb как
//! индекс в блобы, не прямое хранилище тел) решал бы, но это отдельный
//! архитектурный шаг, не который делается сейчас без демонстрации
//! реальной нужды в компактности (DEV_GUIDE, правило 10: диагностировать
//! до того, как чинить то, что не факт что сломано).

use blake2::Blake2bVar;
use blake2::digest::VariableOutput;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub type TokenId = u32;

fn compute_hash(level: u32, children: &[TokenId], child_hashes: impl Fn(TokenId) -> [u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(level.to_be_bytes());
    if level == 0 {
        h.update([children[0] as u8]);
    } else {
        for &c in children {
            h.update(child_hashes(c));
        }
    }
    h.finalize().into()
}

/// Диагностический хеш (blake2b-16, тот же алгоритм и та же схема, что
/// python-эталон использует для id) — НЕ для идентичности/дедупликации
/// (это по-прежнему SHA-256, буква шага 1), только для сравнения
/// гипотезы: даёт ли смена компаратора тай-брейка на "хеш-байты вместо
/// u32-индекса рождения" сближение словаря с python. Кандидат на удаление
/// после того, как эксперимент даст ответ — см. merge::TieBreak.
fn compute_tie_hash(level: u32, children: &[TokenId], child_tie_hashes: impl Fn(TokenId) -> [u8; 16]) -> [u8; 16] {
    use blake2::digest::Update;
    let mut h = Blake2bVar::new(16).expect("16 — допустимый digest_size для blake2b");
    h.update(&level.to_be_bytes());
    if level == 0 {
        h.update(&[children[0] as u8]);
    } else {
        for &c in children {
            h.update(&child_tie_hashes(c));
        }
    }
    let mut out = [0u8; 16];
    h.finalize_variable(&mut out).expect("digest_size совпадает с буфером");
    out
}

/// Внутренний API полки: intern, get, bytes_of. Мутирующих методов над
/// телом токена нет и не будет (append-only) — только `touch` над
/// отдельным, явно не-телесным счётчиком (INVARIANTS.md: тело неизменно,
/// счётчик — восстановимый вид, разные структуры).
///
/// **Шаг 15 (surprise, план 3) добавил два выхода за буквальный список
/// "intern, get, bytes_of. Ничего добавлять" из шага 1** — решение,
/// зафиксированное явно (DEV_GUIDE, правило 2), не тихое расширение:
/// - `lookup` — читает hash_index БЕЗ вставки. Нужен для "распознавания
///   без права рождения" (S-static): проверить, существует ли уже состав,
///   не создавая его, если не существует — то, что `intern` в принципе
///   не может (он либо находит, либо создаёт).
/// - `Clone` — нужен для "холостого прогона рождения" (S-growth):
///   померить, СКОЛЬКО родилось бы на новом входе, не тронув реальный
///   (замороженный) стор — append-only не даёт отката, значит нужна копия
///   до попытки, не отмена после.
#[derive(Clone)]
pub struct Store {
    hashes: Vec<[u8; 32]>,
    hash_index: HashMap<[u8; 32], TokenId>,
    tie_hashes: Vec<[u8; 16]>,
    levels: Vec<u32>,
    children_start: Vec<u32>,
    children_len: Vec<u32>,
    children_arena: Vec<TokenId>,
    counts: Vec<u64>,
    birth_log: Vec<TokenId>,
}

impl Store {
    pub fn new() -> Self {
        Store {
            hashes: Vec::new(),
            hash_index: HashMap::new(),
            tie_hashes: Vec::new(),
            levels: Vec::new(),
            children_start: Vec::new(),
            children_len: Vec::new(),
            children_arena: Vec::new(),
            counts: Vec::new(),
            birth_log: Vec::new(),
        }
    }

    /// Уровень 0 — фабричный, создаётся целиком до чтения любого входа.
    pub fn init_factory(&mut self) {
        for b in 0..256u32 {
            self.intern(0, &[b]);
        }
    }

    pub fn intern(&mut self, level: u32, children: &[TokenId]) -> TokenId {
        let hash = compute_hash(level, children, |c| self.hashes[c as usize]);
        if let Some(&id) = self.hash_index.get(&hash) {
            return id;
        }
        let tie_hash = compute_tie_hash(level, children, |c| self.tie_hashes[c as usize]);
        let id = self.hashes.len() as TokenId;
        let start = self.children_arena.len() as u32;
        self.children_arena.extend_from_slice(children);
        self.hashes.push(hash);
        self.tie_hashes.push(tie_hash);
        self.levels.push(level);
        self.children_start.push(start);
        self.children_len.push(children.len() as u32);
        self.counts.push(0);
        self.birth_log.push(id);
        self.hash_index.insert(hash, id);
        id
    }

    /// Проверить, существует ли уже состав (level, children) — БЕЗ
    /// вставки, если нет. Отличие от `intern`: `intern` создаёт при
    /// отсутствии, `lookup` только читает hash_index. Нужен шагу 15
    /// (S-static — "распознавание без права рождения", см. `surprise.rs`).
    pub fn lookup(&self, level: u32, children: &[TokenId]) -> Option<TokenId> {
        let hash = compute_hash(level, children, |c| self.hashes[c as usize]);
        self.hash_index.get(&hash).copied()
    }

    /// См. compute_tie_hash — диагностический blake2b, не идентичность.
    pub fn tie_hash_of(&self, id: TokenId) -> [u8; 16] {
        self.tie_hashes[id as usize]
    }

    pub fn get(&self, id: TokenId) -> (u32, &[TokenId]) {
        let start = self.children_start[id as usize] as usize;
        let len = self.children_len[id as usize] as usize;
        (self.levels[id as usize], &self.children_arena[start..start + len])
    }

    pub fn level_of(&self, id: TokenId) -> u32 {
        self.levels[id as usize]
    }

    /// Текстовое/байтовое представление — не хранится, вычисляется спуском
    /// по children только по требованию (снапшот/отладка), не в горячем
    /// цикле (INVARIANTS.md).
    pub fn bytes_of(&self, id: TokenId) -> Vec<u8> {
        let (level, children) = self.get(id);
        if level == 0 {
            vec![children[0] as u8]
        } else {
            let mut out = Vec::new();
            for &c in children {
                out.extend(self.bytes_of(c));
            }
            out
        }
    }

    pub fn touch(&mut self, id: TokenId) {
        self.counts[id as usize] += 1;
    }

    pub fn count_of(&self, id: TokenId) -> u64 {
        self.counts[id as usize]
    }

    pub fn hash_of(&self, id: TokenId) -> [u8; 32] {
        self.hashes[id as usize]
    }

    pub fn birth_log(&self) -> &[TokenId] {
        &self.birth_log
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_level0_has_256_tokens_indexed_by_byte_order() {
        let mut store = Store::new();
        store.init_factory();
        assert_eq!(store.len(), 256);
        for b in 0..256u32 {
            assert_eq!(store.level_of(b), 0);
            assert_eq!(store.bytes_of(b), vec![b as u8]);
        }
    }

    #[test]
    fn intern_dedupes_identical_composition() {
        let mut store = Store::new();
        store.init_factory();
        let a = store.intern(1, &[65, 66]);
        let b = store.intern(1, &[65, 66]);
        assert_eq!(a, b);
        assert_eq!(store.len(), 257);
    }

    #[test]
    fn intern_distinguishes_different_children_or_level() {
        let mut store = Store::new();
        store.init_factory();
        let ab = store.intern(1, &[65, 66]);
        let ba = store.intern(1, &[66, 65]);
        assert_ne!(ab, ba);
        let abc = store.intern(1, &[65, 67]);
        assert_ne!(ab, abc);
    }

    #[test]
    fn bytes_of_recurses_through_composition() {
        let mut store = Store::new();
        store.init_factory();
        let ab = store.intern(1, &[b'a' as u32, b'b' as u32]);
        let cd = store.intern(1, &[b'c' as u32, b'd' as u32]);
        let abcd = store.intern(2, &[ab, cd]);
        assert_eq!(store.bytes_of(abcd), b"abcd");
    }

    #[test]
    fn hash_depends_on_child_hash_not_child_index() {
        // Тот же состав, собранный в другом порядке рождения (другой стор),
        // обязан дать тот же id, если бы id считался от индекса — не дал бы.
        let mut store1 = Store::new();
        store1.init_factory();
        let ab1 = store1.intern(1, &[b'a' as u32, b'b' as u32]);

        let mut store2 = Store::new();
        store2.init_factory();
        // рождаем что-то постороннее раньше, чтобы у 'a'/'b' в этом сторе
        // были те же индексы (0..255 из фабрики), но у составного узла
        // индекс отличался бы от store1, если бы что-то родилось раньше.
        let _decoy = store2.intern(1, &[b'x' as u32, b'y' as u32]);
        let ab2 = store2.intern(1, &[b'a' as u32, b'b' as u32]);

        assert_eq!(store1.hash_of(ab1), store2.hash_of(ab2));
        assert_ne!(ab1, ab2, "индексы разные (разный порядок рождения)");
    }
}
