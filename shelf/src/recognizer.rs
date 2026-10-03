//! # StreamingRecognizer — Потоковое распознавание на базе Merkle DAG
//!
//! Реализует инкрементальную свёртку потока сырых байт произвольного размера
//! в последовательность токенов на основе существующего Merkle DAG хранилища (`Store`).
//!
//! Инварианты:
//! 1. **Zero-mutation (Read-only):** Распознаватель не создает новых токенов в `Store` (только `lookup`).
//! 2. **Побитовый детерминизм:** Результат распознавания идентичен независимо от того,
//!    подаются ли байты по 1 байту, произвольными чанками или всем срезом целиком.
//! 3. **Каскадная редукция:** Новые токены редуцируются на стеке до сходимости влево,
//!    обеспечивая эквивалентность каноническому жадному лево-ассоциативному разбору за O(N).

use crate::store::{Store, TokenId};
use std::collections::HashSet;

pub struct StreamingRecognizer<'a> {
    store: &'a Store,
    stack: Vec<TokenId>,
    focused: Option<&'a HashSet<TokenId>>,
}

impl<'a> StreamingRecognizer<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self {
            store,
            stack: Vec::new(),
            focused: None,
        }
    }

    pub fn with_focus(store: &'a Store, focused: &'a HashSet<TokenId>) -> Self {
        Self {
            store,
            stack: Vec::new(),
            focused: Some(focused),
        }
    }

    /// Подать один токен в распознаватель и выполнить каскадную редукцию на стеке.
    pub fn feed_token(&mut self, token: TokenId) {
        self.stack.push(token);
        self.reduce();
    }

    /// Подать срез токенов в распознаватель.
    pub fn feed_tokens(&mut self, tokens: &[TokenId]) {
        for &t in tokens {
            self.feed_token(t);
        }
    }

    /// Подать срез байт в распознаватель (каждый байт транслируется в токен уровня 0).
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let token_id = b as TokenId;
            self.feed_token(token_id);
        }
    }

    /// Каскадная редукция вершины стека до сходимости влево.
    fn reduce(&mut self) {
        loop {
            let len = self.stack.len();
            if len < 2 {
                break;
            }
            let a = self.stack[len - 2];
            let b = self.stack[len - 1];
            let level = self.store.level_of(a).max(self.store.level_of(b)) + 1;
            if let Some(composite_id) = self.store.lookup(level, &[a, b]) {
                let usable = self.focused.is_none_or(|f| f.contains(&composite_id));
                if usable {
                    self.stack.pop();
                    self.stack.pop();
                    self.stack.push(composite_id);
                    continue;
                }
            }
            break;
        }
    }

    /// Текущий срез токенов в стеке.
    pub fn as_slice(&self) -> &[TokenId] {
        &self.stack
    }

    /// Число токенов в стеке.
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Очистить стек.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// Забрать накопленные токены, очистив внутренний стек.
    pub fn drain(&mut self) -> Vec<TokenId> {
        std::mem::take(&mut self.stack)
    }

    /// Завершить поток и вернуть итоговую последовательность токенов.
    pub fn finish(self) -> Vec<TokenId> {
        self.stack
    }

    /// Одномоментное распознавание среза байт.
    pub fn recognize(store: &'a Store, bytes: &[u8]) -> Vec<TokenId> {
        Self::recognize_focused(store, bytes, None)
    }

    /// Одномоментное распознавание среза байт с учетом активного фокуса.
    pub fn recognize_focused(
        store: &'a Store,
        bytes: &[u8],
        focused: Option<&'a HashSet<TokenId>>,
    ) -> Vec<TokenId> {
        let mut recognizer = match focused {
            Some(f) => Self::with_focus(store, f),
            None => Self::new(store),
        };
        recognizer.feed_bytes(bytes);
        recognizer.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::TieBreak;
    use crate::wheel::WheelState;

    fn build_test_store() -> (Store, TokenId, TokenId, TokenId) {
        let mut store = Store::new();
        store.init_factory();
        let a = b'a' as TokenId;
        let b = b'b' as TokenId;
        let c = b'c' as TokenId;
        let ab = store.intern(1, &[a, b]);
        let bc = store.intern(1, &[b, c]);
        let abc = store.intern(2, &[ab, c]);
        (store, ab, bc, abc)
    }

    #[test]
    fn test_streaming_matches_one_shot() {
        let (store, _ab, _bc, abc) = build_test_store();

        // 1. One shot
        let tokens_one_shot = StreamingRecognizer::recognize(&store, b"abc");
        assert_eq!(tokens_one_shot, vec![abc]);

        // 2. Feed byte by byte
        let mut recognizer = StreamingRecognizer::new(&store);
        recognizer.feed_bytes(b"a");
        recognizer.feed_bytes(b"b");
        recognizer.feed_bytes(b"c");
        let tokens_stream = recognizer.finish();
        assert_eq!(tokens_stream, vec![abc]);
    }

    #[test]
    fn test_arbitrary_chunk_sizes_equivalence() {
        let mut wheel = WheelState::new();
        wheel.turn(b"the cat sat on the mat. the cat sat on the mat.", 2, TieBreak::BirthOrder);
        let store = &wheel.store;

        let input = b"the cat sat on the mat. the cat sat on the mat.";

        let canonical = StreamingRecognizer::recognize(store, input);

        // По 1 байту
        let mut rec1 = StreamingRecognizer::new(store);
        for &b in input {
            rec1.feed_bytes(&[b]);
        }
        assert_eq!(rec1.finish(), canonical, "не совпадает при подаче по 1 байту");

        // Чанками по 3 байта
        let mut rec3 = StreamingRecognizer::new(store);
        for chunk in input.chunks(3) {
            rec3.feed_bytes(chunk);
        }
        assert_eq!(rec3.finish(), canonical, "не совпадает при чанках по 3 байта");

        // Чанками по 7 байт
        let mut rec7 = StreamingRecognizer::new(store);
        for chunk in input.chunks(7) {
            rec7.feed_bytes(chunk);
        }
        assert_eq!(rec7.finish(), canonical, "не совпадает при чанках по 7 байт");
    }

    #[test]
    fn test_focused_filter_respects_allowed_set() {
        let (store, ab, _bc, abc) = build_test_store();

        // Если разрешен только ab, abc не может быть собран
        let mut focused = HashSet::new();
        focused.insert(ab);

        let tokens = StreamingRecognizer::recognize_focused(&store, b"abc", Some(&focused));
        assert_eq!(tokens, vec![ab, b'c' as TokenId]);

        // Если разрешен и abc
        focused.insert(abc);
        let tokens_full = StreamingRecognizer::recognize_focused(&store, b"abc", Some(&focused));
        assert_eq!(tokens_full, vec![abc]);
    }

    #[test]
    fn test_drain_and_continue() {
        let (store, ab, _bc, _abc) = build_test_store();
        let mut rec = StreamingRecognizer::new(&store);

        rec.feed_bytes(b"ab");
        let out1 = rec.drain();
        assert_eq!(out1, vec![ab]);
        assert!(rec.is_empty());

        rec.feed_bytes(b"ab");
        let out2 = rec.finish();
        assert_eq!(out2, vec![ab]);
    }
}
