//! Шаг 8 (A1, план 3): экстракторы контекста для перевёрнутой библиотеки.
//!
//! Событие встречи в контексте — тройка (token, context_token, weight).
//!
//! **Направленность — решение, не покрытое планом явно, зафиксировано
//! здесь (DEV_GUIDE, правило 2):** Neighbor и Window эмитят пару в обе
//! стороны — соседство симметрично по смыслу (a рядом с b ⇔ b рядом с a).
//! Parent направлен по построению (ребёнок → родитель, не наоборот) —
//! родитель не встречается "в контексте" своего ребёнка, это другое
//! отношение, не переиспользование той же симметрии.
//!
//! **Итог эксперимента A4 (план 3, Правка 1, 2026-07-18):** первый прогон
//! протокола (три экстрактора на ФИНАЛЬНОЙ свёрнутой последовательности)
//! показал, что все три — не равноправные кандидаты для одного пайплайна.
//! Роли развелись по итогу:
//! - `Neighbor` закрыт СТРУКТУРНО (не проигранной метрикой): смежность
//!   выше порога рождения уже поглощается прямой полкой в композиты, на
//!   финальной последовательности от неё остаётся почти ноль сигнала
//!   (макс. вес пары в COO — 2 из 405К различных пар на bhagavad_gita.txt,
//!   при частотном полу 3 — 0 оценённых пар вообще) плюс артефакт
//!   собственной симметризации (одно вхождение "a b" + одно отдельное "b a"
//!   в другом месте текста дают направленной паре (a,b) вес 2, не будучи
//!   повторной встречей одной и той же смежности). Проверено: это не баг
//!   каскада — повторный прогон `merge::run` на собственном выводе не
//!   находит новых слияний (`examples/rerun_diagnostic.rs`). Оставлен в
//!   коде как исторически первый, поучительный и корректный экстрактор —
//!   не используется в пайплайне A4+.
//! - `Window` — победитель, но НЕ с диапазоном ±1..N: дистанция 1 — то,
//!   что каскад разбирает в этом же поколении (та же юрисдикция, что
//!   поглотила Neighbor), окно должно считать только то, что каскад
//!   принципиально не может съесть. Диапазон — `Window::range(min, max)`,
//!   принятая конфигурация — `range(2, 5)`.
//! - `Parent` — не источник для перевёрнутой библиотеки вообще: рёбра
//!   состава уже лежат на прямой полке (`Store::get`), дублировать их в
//!   `InvertedStore`/`cooc.rs` — хранить один факт дважды. Логика остаётся
//!   здесь, т.к. пригодится шагу 10 (граф блужданий читает рёбра состава
//!   напрямую с прямой полки, тем же способом).

use crate::store::{Store, TokenId};

pub trait ContextExtractor {
    /// (token, context_token, weight). Вес — число "сырых встреч" этой
    /// пары, которые дальше суммирует COO-аккумулятор (шаг 9); не
    /// обязательно 1 на вызов (см. `Parent`, где одно ребро несёт вес
    /// count_of(родителя) вместо пере-эмиссии много раз).
    fn extract(&self, store: &Store, sequence: &[TokenId]) -> Vec<(TokenId, TokenId, u64)>;
}

/// Оставить только события, где ОБА токена (`token` и `context`) имеют
/// уровень >= `min_level` — общий фильтр, применим к выводу любого
/// экстрактора, не специфика конкретного (план 3, Правка 1, Р-А4-1:
/// "байтовый и первый уровни дают шум посимвольной статистики").
pub fn filter_by_min_level(
    store: &Store,
    events: Vec<(TokenId, TokenId, u64)>,
    min_level: u32,
) -> Vec<(TokenId, TokenId, u64)> {
    events
        .into_iter()
        .filter(|&(token, context, _)| {
            store.level_of(token) >= min_level && store.level_of(context) >= min_level
        })
        .collect()
}

/// Контекст = непосредственный сосед в последовательности. Симметрично:
/// на каждую соседнюю пару эмитятся оба направления, вес 1 на вхождение.
///
/// ЗАКРЫТ СТРУКТУРНО решением A4 (см. шапку модуля) — не используется в
/// пайплайне перевёрнутой библиотеки. Оставлен как корректный,
/// протестированный, поучительный экстрактор.
pub struct Neighbor;

impl ContextExtractor for Neighbor {
    fn extract(&self, _store: &Store, sequence: &[TokenId]) -> Vec<(TokenId, TokenId, u64)> {
        let mut out = Vec::with_capacity(sequence.len().saturating_sub(1) * 2);
        for w in sequence.windows(2) {
            let (a, b) = (w[0], w[1]);
            out.push((a, b, 1));
            out.push((b, a, 1));
        }
        out
    }
}

/// Контекст = непосредственный родитель по составу (ребёнок → родитель).
/// Не использует `sequence` — интринсик самой полки: для каждого
/// составного токена (level >= 1) каждый его непосредственный ребёнок
/// "встречается в контексте" этого родителя с весом = сколько раз
/// родитель реально встретился в потоке (`store.count_of`). Родители с
/// нулевым счётчиком (интернированы, но не тронуты `touch`) пропускаются
/// — реального контекста не было, только запись в теле.
///
/// НЕ ИСТОЧНИК для перевёрнутой библиотеки (решение A4, см. шапку модуля):
/// эти рёбра уже лежат на прямой полке, `InvertedStore`/`cooc.rs` их не
/// принимают. Логика остаётся — шаг 10 (граф блужданий) читает рёбра
/// состава этим же способом напрямую со `Store`.
pub struct Parent;

impl ContextExtractor for Parent {
    fn extract(&self, store: &Store, _sequence: &[TokenId]) -> Vec<(TokenId, TokenId, u64)> {
        let mut out = Vec::new();
        for id in 0..store.len() as TokenId {
            let (level, children) = store.get(id);
            if level == 0 {
                continue;
            }
            let weight = store.count_of(id);
            if weight == 0 {
                continue;
            }
            for &c in children {
                out.push((c, id, weight));
            }
        }
        out
    }
}

/// Контекст = все токены на дистанции `min..=max` позиций от данной (в
/// обе стороны). Симметрично, как Neighbor. `min=1` включает
/// непосредственное соседство (историческое поведение); принятая для A4
/// конфигурация — `range(2, 5)`, дистанция 1 сознательно исключена (это
/// юрисдикция библиотекаря в текущем поколении — см. шапку модуля).
pub struct Window {
    pub min: usize,
    pub max: usize,
}

impl Window {
    /// `±N` от позиции, то есть `range(1, n)` — историческое поведение,
    /// сохранено ради обратной совместимости и как частный случай.
    pub fn new(n: usize) -> Self {
        Self::range(1, n)
    }

    pub fn range(min: usize, max: usize) -> Self {
        assert!(min >= 1, "дистанция 0 — это сама позиция, не контекст");
        assert!(max >= min, "max обязан быть не меньше min");
        Window { min, max }
    }
}

impl ContextExtractor for Window {
    fn extract(&self, _store: &Store, sequence: &[TokenId]) -> Vec<(TokenId, TokenId, u64)> {
        let len = sequence.len();
        let mut out = Vec::new();
        for i in 0..len {
            let lo = i.saturating_sub(self.max);
            let hi = (i + self.max + 1).min(len);
            for j in lo..hi {
                if j == i {
                    continue;
                }
                let distance = i.abs_diff(j);
                if distance >= self.min && distance <= self.max {
                    out.push((sequence[i], sequence[j], 1));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbor_emits_both_directions_weight_one() {
        let store = Store::new();
        let seq = vec![1u32, 2, 3];
        let mut got = Neighbor.extract(&store, &seq);
        got.sort();
        let mut want = vec![(1, 2, 1), (2, 1, 1), (2, 3, 1), (3, 2, 1)];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn neighbor_on_short_sequence_emits_nothing() {
        let store = Store::new();
        assert!(Neighbor.extract(&store, &[]).is_empty());
        assert!(Neighbor.extract(&store, &[7]).is_empty());
    }

    #[test]
    fn window_2_matches_hand_computed_pairs() {
        let store = Store::new();
        let seq = vec![10u32, 20, 30, 40];
        let mut got = Window::new(2).extract(&store, &seq);
        got.sort();
        let mut want = vec![
            (10, 20, 1),
            (10, 30, 1),
            (20, 10, 1),
            (20, 30, 1),
            (20, 40, 1),
            (30, 10, 1),
            (30, 20, 1),
            (30, 40, 1),
            (40, 20, 1),
            (40, 30, 1),
        ];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn window_1_matches_neighbor() {
        let store = Store::new();
        let seq = vec![1u32, 2, 3, 4];
        let mut a = Neighbor.extract(&store, &seq);
        let mut b = Window::new(1).extract(&store, &seq);
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }

    #[test]
    fn window_range_2_5_excludes_distance_1() {
        // seq = [10,20,30,40,50,60,70], позиция 3 (значение 40): дистанция
        // 1 (позиции 2,4 -> 30,50) обязана отсутствовать, дистанция 2..5
        // (позиции 0..2,4..8, тут только 0,1,2,4,5,6 в границах) — входит.
        let store = Store::new();
        let seq: Vec<u32> = vec![10, 20, 30, 40, 50, 60, 70];
        let got = Window::range(2, 5).extract(&store, &seq);

        // Из позиции 3 (значение 40): дистанция 1 (30,50) не должна
        // встретиться ни разу.
        assert!(
            !got.contains(&(40, 30, 1)) && !got.contains(&(40, 50, 1)),
            "дистанция 1 обязана быть исключена из range(2,5)"
        );
        // Дистанция 2 из позиции 3: позиция 1 (20) и позиция 5 (60).
        assert!(got.contains(&(40, 20, 1)));
        assert!(got.contains(&(40, 60, 1)));
        // Дистанция 3 из позиции 3: позиция 0 (10) и позиция 6 (70).
        assert!(got.contains(&(40, 10, 1)));
        assert!(got.contains(&(40, 70, 1)));
    }

    #[test]
    fn window_range_min_bound_is_inclusive_max_bound_is_inclusive() {
        let store = Store::new();
        let seq: Vec<u32> = (0..10).collect();
        let w = Window::range(2, 5);
        let got = w.extract(&store, &seq);
        // Из позиции 5: дистанция ровно 2 (позиции 3,7) и ровно 5
        // (позиции 0, но 10 вне границ) обязаны присутствовать.
        assert!(got.contains(&(5, 3, 1)), "дистанция 2 (нижняя граница) обязана войти");
        assert!(got.contains(&(5, 7, 1)), "дистанция 2 (нижняя граница) обязана войти");
        assert!(got.contains(&(5, 0, 1)), "дистанция 5 (верхняя граница) обязана войти");
        assert!(!got.contains(&(5, 4, 1)), "дистанция 1 обязана быть исключена");
        assert!(!got.contains(&(5, 6, 1)), "дистанция 1 обязана быть исключена");
    }

    #[test]
    fn filter_by_min_level_keeps_only_pairs_where_both_sides_qualify() {
        let mut store = Store::new();
        store.init_factory();
        let ab = store.intern(1, &[b'a' as u32, b'b' as u32]); // level 1
        let cd = store.intern(1, &[b'c' as u32, b'd' as u32]); // level 1
        let abcd = store.intern(2, &[ab, cd]); // level 2

        let events = vec![
            (abcd, abcd, 1),          // оба level>=2 -> проходит
            (abcd, ab, 1),            // ab level 1 -> не проходит
            (ab, cd, 1),              // оба level 1 -> не проходит
            (b'a' as u32, abcd, 1),   // байт уровня 0 -> не проходит
        ];
        let filtered = filter_by_min_level(&store, events, 2);
        assert_eq!(filtered, vec![(abcd, abcd, 1)]);
    }

    #[test]
    fn parent_extracts_children_weighted_by_parent_count() {
        let mut store = Store::new();
        store.init_factory();
        let ab = store.intern(1, &[b'a' as u32, b'b' as u32]);
        store.touch(ab);
        store.touch(ab);
        store.touch(ab); // count_of(ab) == 3

        let cd = store.intern(1, &[b'c' as u32, b'd' as u32]); // не тронут — count 0

        let mut got = Parent.extract(&store, &[]);
        got.sort();

        // ab: два ребра (a,ab,3) и (b,ab,3). cd: не встречается вообще
        // (count 0 -> отфильтрован).
        assert!(got.contains(&(b'a' as u32, ab, 3)));
        assert!(got.contains(&(b'b' as u32, ab, 3)));
        assert!(!got.iter().any(|&(_, parent, _)| parent == cd));
        assert_eq!(got.len(), 2, "уровень 0 и непосещённый cd не должны попасть в вывод");
    }

    #[test]
    fn parent_ignores_level_zero_factory_tokens() {
        let mut store = Store::new();
        store.init_factory();
        for b in 0..256u32 {
            store.touch(b);
        }
        let got = Parent.extract(&store, &[]);
        assert!(got.is_empty(), "уровень 0 не имеет родителя, только сам является ребёнком");
    }
}
