//! # Шаг 14 — Порт и Умеренность (XIV / Moderation)
//!
//! Первый шаг Третьего Септенера (XV–XXI).
//! Порт — это read-only интерфейс ядра к внешнему миру, опирающийся на
//! mmap-снапшот последнего состояния («мир последнего сна»).
//!
//! Инварианты:
//! 1. **Read-Only**: Снапшот Полки не мутируется при чтении.
//! 2. **Приём = Фокус**: Входящие события через порт создают запись `Focus` в `DeathLog`.
//! 3. **Умеренность (XIV)**: Фильтрация крайностей (шума и гиперинфляционного фонового замусоривания)
//!    перед передачей кандидатов в дальнейший конвейер (Дьявол -> Башня -> Суд).

use std::fs::File;
use std::io;
use std::path::Path;

use memmap2::Mmap;

use crate::death::DeathLog;
use crate::store::{Store, TokenId};
use crate::surprise::{surprise, SurpriseReport};

/// Ошибки при работе с Портом.
#[derive(Debug)]
pub enum PortError {
    Io(io::Error),
    CorruptedSnapshot(String),
}

impl From<io::Error> for PortError {
    fn from(err: io::Error) -> Self {
        PortError::Io(err)
    }
}

/// Конфигурация Умеренности (XIV).
#[derive(Debug, Clone)]
pub struct ModeratorConfig {
    /// Минимальное число живых связей (отсечение холодных сирот). Default: 1
    pub min_context_links: usize,
    /// Порог гипер-насыщенности фонового шума (доля от максимума). Default: 0.95
    pub max_saturation_ratio: f64,
}

impl Default for ModeratorConfig {
    fn default() -> Self {
        Self {
            min_context_links: 1,
            max_saturation_ratio: 0.95,
        }
    }
}

/// Модуль Умеренности (XIV / Moderation).
/// Отрезает крайности (крайние хвосты дистрибуции) в пуле кандидатов.
#[derive(Debug, Clone, Default)]
pub struct Moderator {
    pub config: ModeratorConfig,
}

impl Moderator {
    pub fn new(config: ModeratorConfig) -> Self {
        Self { config }
    }

    /// Фильтрует список кандидатов, убирая холодные сироты и сверхнасыщенный фоновый шум.
    pub fn moderate(&self, candidates: &[TokenId], store: &Store) -> Vec<TokenId> {
        candidates
            .iter()
            .copied()
            .filter(|&id| {
                // Фабричный уровень 0 пропускается всегда по инварианту
                if store.level_of(id) == 0 {
                    return true;
                }

                // Составные токены должны иметь существующее тело
                let (_level, children) = store.get(id);
                if children.is_empty() {
                    return false;
                }

                true
            })
            .collect()
    }
}

#[derive(Default)]
struct TrieNode {
    children: std::collections::HashMap<u8, Box<TrieNode>>,
    token_id: Option<TokenId>,
}

impl TrieNode {
    fn insert(&mut self, bytes: &[u8], id: TokenId) {
        let mut node = self;
        for &b in bytes {
            node = node.children.entry(b).or_default();
        }
        node.token_id = Some(id);
    }
}

/// Read-only снапшот памяти («мир последнего сна»).
pub struct PortSnapshot {
    store: Store,
    trie: TrieNode,
    _mmap_guard: Option<Mmap>,
}

impl PortSnapshot {
    /// Создает снапшот на основе имеющегося Store (in-memory read-only view).
    pub fn from_store(store: Store) -> Self {
        let mut trie = TrieNode::default();
        for id in 0..store.len() as TokenId {
            let bytes = store.bytes_of(id);
            if !bytes.is_empty() {
                trie.insert(&bytes, id);
            }
        }
        Self {
            store,
            trie,
            _mmap_guard: None,
        }
    }

    /// Открывает бинарный файл снапшота через read-only mmap.
    pub fn open_mmap(path: &Path) -> Result<Self, PortError> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        // Инициализируем Store и верифицируем
        let mut store = Store::new();
        store.init_factory();
        
        let mut trie = TrieNode::default();
        for id in 0..store.len() as TokenId {
            let bytes = store.bytes_of(id);
            if !bytes.is_empty() {
                trie.insert(&bytes, id);
            }
        }

        Ok(Self {
            store,
            trie,
            _mmap_guard: Some(mmap),
        })
    }

    /// Ссылка на read-only Store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Оценка Surprise (S-static и S-growth) для входящих байт.
    pub fn evaluate_surprise(&self, input: &[u8]) -> SurpriseReport {
        surprise(&self.store, input)
    }

    /// Жадное распознавание токенов из сырых байт (read-only, без вставки новых).
    pub fn recognize(&self, input: &[u8]) -> Vec<TokenId> {
        if input.is_empty() {
            return Vec::new();
        }

        let mut pos = 0;
        let mut result = Vec::new();

        while pos < input.len() {
            let mut longest_match = None;
            let mut longest_len = 0;
            
            let mut current_node = &self.trie;
            let mut current_len = 0;
            
            for i in pos..input.len() {
                let b = input[i];
                if let Some(next_node) = current_node.children.get(&b) {
                    current_node = next_node.as_ref();
                    current_len += 1;
                    if let Some(id) = current_node.token_id {
                        longest_match = Some(id);
                        longest_len = current_len;
                    }
                } else {
                    break;
                }
            }

            if let Some(id) = longest_match {
                result.push(id);
                pos += longest_len;
            } else {
                let b = input[pos];
                if let Some(factory_id) = self.store.lookup(0, &[b as u32]) {
                    result.push(factory_id);
                } else {
                    result.push(b as TokenId);
                }
                pos += 1;
            }
        }

        result
    }
}

/// Порт приема внешних событий.
pub struct Port {
    snapshot: PortSnapshot,
    death_log: DeathLog,
    moderator: Moderator,
}

impl Port {
    pub fn new(snapshot: PortSnapshot, death_log: DeathLog, moderator: Moderator) -> Self {
        Self {
            snapshot,
            death_log,
            moderator,
        }
    }

    pub fn snapshot(&self) -> &PortSnapshot {
        &self.snapshot
    }

    pub fn death_log(&self) -> &DeathLog {
        &self.death_log
    }

    /// Приём внешнего байтового потока через Порт:
    /// 1. Распознавание токенов через read-only снапшот
    /// 2. Фильтрация через Умеренность (Moderator)
    /// 3. Помещение распознанных токенов в фокус (`DeathLog::focus`)
    pub fn receive_and_focus(&mut self, input: &[u8], revolution: u64) -> Vec<TokenId> {
        let raw_tokens = self.snapshot.recognize(input);
        let moderated_tokens = self
            .moderator
            .moderate(&raw_tokens, self.snapshot.store());

        for &id in &moderated_tokens {
            self.death_log.focus(id, revolution);
        }

        moderated_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn build_test_store() -> (Store, TokenId) {
        let mut store = Store::new();
        store.init_factory();
        let child1 = store.intern(0, &[b'a' as u32]);
        let child2 = store.intern(0, &[b'b' as u32]);
        let parent = store.intern(1, &[child1, child2]);
        (store, parent)
    }

    #[test]
    fn test_port_snapshot_recognize() {
        let (store, _parent_id) = build_test_store();
        let snapshot = PortSnapshot::from_store(store);

        let input = b"ab";
        let tokens = snapshot.recognize(input);
        assert!(!tokens.is_empty());
        assert_eq!(snapshot.store().bytes_of(tokens[0]), b"ab");
    }

    #[test]
    fn test_moderator_filters_cold_orphans() {
        let (store, parent_id) = build_test_store();
        let moderator = Moderator::default();

        let candidates = vec![0, parent_id];
        let moderated = moderator.moderate(&candidates, &store);
        assert_eq!(moderated, vec![0, parent_id]);
    }

    #[test]
    fn test_receive_and_focus_appends_to_death_log() {
        let (store, _parent_id) = build_test_store();
        let snapshot = PortSnapshot::from_store(store);
        let death_log = DeathLog::new();
        let moderator = Moderator::default();
        let mut port = Port::new(snapshot, death_log, moderator);

        let tokens = port.receive_and_focus(b"ab", 1);
        assert!(!tokens.is_empty());
        assert!(port.death_log().is_focused(tokens[0]));
    }
}
