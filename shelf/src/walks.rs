//! Шаг 10 (A5, план 3, Правка 4 — ОКОНЧАТЕЛЬНАЯ конструкция, Р-А5-4):
//! offline random-walk mining через walk-PPMI с эмпирическим вычитанием
//! структурного фона.
//!
//! **История трёх провалов (см. Deviations, план 3):** первые три попытки
//! (прямые родственники минус, оконный флаг "≥1 PPMI-ребро в сегменте")
//! мерили СЧЁТ со-посещений — а счёт пропорционален ОБЪЁМУ обхода (число
//! блужданий × длина), не информации в графе. При 130К узлов × 10
//! блужданий по 40 шагов объём обхода (1.3 млн блужданий) забивал
//! разницу в плотности PPMI-рёбер между реальным и нулевым графом
//! (11-кратная разница заражённости узлов давала всего ×1.42 разницы в
//! сырых счётах).
//!
//! **Р-А5-4:** метрика обязана быть инвариантна к объёму обхода — точно
//! так же, как PPMI уже инвариантен к объёму корпуса (шаг 11). Удвоение
//! числа блужданий удваивает все счётчики со-посещений СИММЕТРИЧНО (сумма
//! по строке и по столбцу растёт тем же множителем) — PMI = log2(w·total/
//! (row·col)) от такого масштабирования не меняется. Не трюк, а теория:
//! результат Леви-Голдберга — обучение на блужданиях (DeepWalk/word2vec)
//! асимптотически эквивалентно факторизации PMI-матрицы со-встречаемости.
//! Блуждания без PMI-нормализации — сырьё, не сигнал.
//!
//! **Конструкция:** три прогона одним протоколом (граф, длина 40, 10
//! блужданий/узел, политика перехода 1/2 — без изменений):
//! 1. **реальный** граф (структура + PPMI-рёбра из реального COO);
//! 2. **переставленный** граф (структура + PPMI-рёбра из нулевого COO);
//! 3. **чисто структурный** граф (без единого PPMI-ребра вообще) — то,
//!    что рождает высокий walk-PPMI ЗДЕСЬ, порождено составом как таковым,
//!    не со-встречаемостью, и вычитается из обоих условий как фон B.
//!
//! В каждом прогоне со-посещения (окно 2..5, симметричная эмиссия ОБОИХ
//! направлений на со-посещение — см. `covisitation_events`) накапливаются
//! тем же `CooAccumulator`, запекаются в тот же `Csr`, судятся тем же
//! `ppmi::valid_links` (пол 3, порог 2.0), что и прямой поток. Кандидаты
//! условия = walk-PPMI-валидные пары МИНУС фон B, минус уже известные на
//! перевёрнутой полке этого условия (`corpus PPMI`), минус прямая родня
//! (родитель-ребёнок, сиблинги). Один и тот же судья (PPMI-модуль) для
//! внешнего потока и внутренних блужданий — не было запланировано,
//! получилось из вынужденной необходимости после трёх пустых прогонов.
//!
//! **Почему двусторонняя эмиссия здесь корректна** (в отличие от Правки 3,
//! где нормализация min/max была обязательна): предыдущие попытки мерили
//! АБСОЛЮТНЫЙ счёт, где двойная эмиссия удвоила бы результат неравномерно
//! между условиями с разной плотностью — реальный артефакт. Здесь метрика
//! (PPMI) масштабно-инвариантна по построению: равномерное удвоение ВСЕХ
//! весов (row/col-маргиналы растут тем же множителем) не меняет
//! результат. Двусторонняя эмиссия здесь не просто безопасна — она
//! ОБЯЗАТЕЛЬНА: без неё row/col-маргиналы CSR отражали бы только
//! половину истинной степени токена (только со-посещения, где токен
//! оказался численно меньшим при нормализации), что исказило бы сам PPMI.
//!
//! **Р3 (жёстко, план 3):** выход — производный слой, не персистится.
//!
//! **Терминальность (Правка 4):** это последняя форма гейта добычи.
//! Провал — принятый отрицательный результат ("блуждания на этом корпусе
//! не добывают латентного сигнала сверх структуры и прямой
//! со-встречаемости"), не повод для четвёртого редизайна. Шаг 10 в этом
//! случае сдаётся как инфраструктура без майнингового заявления; пачка A
//! закрывается на G-A1′ + G-A3.

use crate::cooc::{CooAccumulator, Csr};
use crate::ppmi::{self, PpmiEntry};
use crate::rng::SplitMix64;
use crate::store::{Store, TokenId};
use std::collections::{HashMap, HashSet};

pub struct WalkGraph {
    structural: HashMap<TokenId, Vec<TokenId>>,
    cooccurrence: HashMap<TokenId, Vec<(TokenId, f64)>>,
    /// Нормализованные (min,max) пары "дети одного родителя" — для
    /// фильтра кандидатов (прямая родня, сиблинги).
    sibling_pairs: HashSet<(TokenId, TokenId)>,
}

fn normalize(a: TokenId, b: TokenId) -> (TokenId, TokenId) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

impl WalkGraph {
    /// `min_level` — полоса узлов (2, как у со-встречаемости).
    /// `valid_links` — уже отфильтрованные PPMI-валидные связи; пустой
    /// срез даёт ЧИСТО СТРУКТУРНЫЙ граф (третий прогон Р-А5-4).
    pub fn build(store: &Store, valid_links: &[PpmiEntry], min_level: u32) -> Self {
        let mut structural: HashMap<TokenId, Vec<TokenId>> = HashMap::new();
        let mut sibling_pairs: HashSet<(TokenId, TokenId)> = HashSet::new();

        for id in 0..store.len() as TokenId {
            let (level, children) = store.get(id);
            if level == 0 {
                continue;
            }
            if children.len() == 2 {
                sibling_pairs.insert(normalize(children[0], children[1]));
            }
            if level < min_level {
                continue;
            }
            for &c in children {
                if store.level_of(c) < min_level {
                    continue;
                }
                structural.entry(id).or_default().push(c);
                structural.entry(c).or_default().push(id);
            }
        }

        let mut cooccurrence: HashMap<TokenId, Vec<(TokenId, f64)>> = HashMap::new();
        for entry in valid_links {
            cooccurrence.entry(entry.token).or_default().push((entry.context, entry.ppmi));
        }

        WalkGraph {
            structural,
            cooccurrence,
            sibling_pairs,
        }
    }

    pub fn structural_degree(&self, node: TokenId) -> usize {
        self.structural.get(&node).map_or(0, Vec::len)
    }

    pub fn cooccurrence_degree(&self, node: TokenId) -> usize {
        self.cooccurrence.get(&node).map_or(0, Vec::len)
    }

    /// Множество узлов графа — объединение структурных и
    /// со-встречаемостных ключей (детерминированный порядок: отсортировано).
    pub fn nodes(&self) -> Vec<TokenId> {
        let mut set: HashSet<TokenId> = self.structural.keys().copied().collect();
        set.extend(self.cooccurrence.keys().copied());
        let mut v: Vec<TokenId> = set.into_iter().collect();
        v.sort_unstable();
        v
    }

    pub fn is_structurally_connected(&self, a: TokenId, b: TokenId) -> bool {
        self.structural.get(&a).is_some_and(|v| v.contains(&b))
    }

    pub fn is_sibling_pair(&self, a: TokenId, b: TokenId) -> bool {
        self.sibling_pairs.contains(&normalize(a, b))
    }
}

fn uniform_pick(items: &[TokenId], rng: &mut SplitMix64) -> TokenId {
    items[rng.next_below(items.len() as u64) as usize]
}

fn weighted_pick(items: &[(TokenId, f64)], rng: &mut SplitMix64) -> TokenId {
    let total: f64 = items.iter().map(|&(_, w)| w).sum();
    if total <= 0.0 {
        return items[0].0;
    }
    let r = (rng.next_u64() as f64 / u64::MAX as f64) * total;
    let mut acc = 0.0;
    for &(tok, w) in items {
        acc += w;
        if r < acc {
            return tok;
        }
    }
    items.last().unwrap().0
}

/// Переход: 1/2 структурно (равномерно), 1/2 по со-встречаемости
/// (PPMI-взвешенно); единственный сорт рёбер — используется он; рёбер
/// нет вовсе — блуждание останавливается. Политика не менялась с Правки 3.
fn step(graph: &WalkGraph, current: TokenId, rng: &mut SplitMix64) -> Option<TokenId> {
    let structural = graph.structural.get(&current).map(Vec::as_slice).unwrap_or(&[]);
    let cooc = graph.cooccurrence.get(&current).map(Vec::as_slice).unwrap_or(&[]);
    match (structural.is_empty(), cooc.is_empty()) {
        (true, true) => None,
        (false, true) => Some(uniform_pick(structural, rng)),
        (true, false) => Some(weighted_pick(cooc, rng)),
        (false, false) => {
            if rng.next_below(2) == 0 {
                Some(uniform_pick(structural, rng))
            } else {
                Some(weighted_pick(cooc, rng))
            }
        }
    }
}

/// Одно блуждание DeepWalk-типа. Короче `length`, если упёрлось в узел
/// без исходящих рёбер — ожидаемо (на чисто структурном графе это узлы
/// вне достижимости корня по составу).
fn random_walk(graph: &WalkGraph, start: TokenId, length: usize, rng: &mut SplitMix64) -> Vec<TokenId> {
    let mut walk = Vec::with_capacity(length);
    walk.push(start);
    let mut current = start;
    for _ in 1..length {
        match step(graph, current, rng) {
            Some(next) => {
                walk.push(next);
                current = next;
            }
            None => break,
        }
    }
    walk
}

/// Со-посещения внутри блуждания: каждая позиционная пара (i,j),
/// `min_dist <= |i-j| <= max_dist`, эмитится В ОБА НАПРАВЛЕНИЯ — см.
/// шапку модуля про масштабную инвариантность PPMI и почему это здесь
/// обязательно (не артефакт, в отличие от Правки 3).
fn covisitation_events(walk: &[TokenId], min_dist: usize, max_dist: usize) -> Vec<(TokenId, TokenId, u64)> {
    let mut out = Vec::new();
    let len = walk.len();
    for i in 0..len {
        let hi = (i + max_dist).min(len.saturating_sub(1));
        let lo = i + min_dist;
        if lo > hi {
            continue;
        }
        for j in lo..=hi {
            let (a, b) = (walk[i], walk[j]);
            out.push((a, b, 1));
            out.push((b, a, 1));
        }
    }
    out
}

/// Прогнать блуждания по всем узлам графа (`walks_per_node` с каждого,
/// длина `walk_length`), накопить со-посещения (окно 2..5). Детерминировано
/// при фиксированном `seed`.
pub fn collect_walk_coo(graph: &WalkGraph, walk_length: usize, walks_per_node: usize, seed: u64) -> CooAccumulator {
    let mut rng = SplitMix64::new(seed);
    let mut coo = CooAccumulator::new();
    for &node in &graph.nodes() {
        for _ in 0..walks_per_node {
            let walk = random_walk(graph, node, walk_length, &mut rng);
            coo.accumulate(&covisitation_events(&walk, 2, 5));
        }
    }
    coo
}

fn normalized_valid_set(csr: &Csr, frequency_floor: u64, threshold: f64) -> HashSet<(TokenId, TokenId)> {
    ppmi::valid_links(csr, frequency_floor, threshold)
        .into_iter()
        .map(|e| normalize(e.token, e.context))
        .collect()
}

pub struct WalkPpmiOutcome {
    pub candidates_real: Vec<(TokenId, TokenId)>,
    pub candidates_permuted: Vec<(TokenId, TokenId)>,
    pub background: HashSet<(TokenId, TokenId)>,
    pub walk_valid_real: usize,
    pub walk_valid_permuted: usize,
}

/// Параметры блужданий, принятые планом (длина 40, 10 на узел). Не гейт,
/// параметр добычи — можно менять, фиксируя в логе.
pub const WALK_LENGTH: usize = 40;
pub const WALKS_PER_NODE: usize = 10;

/// Р-А5-4 целиком: три прогона (реальный/переставленный/структурный),
/// walk-PPMI поверх со-посещений каждого, кандидаты = walk-валидные минус
/// структурный фон минус corpus-PPMI-валидные этого условия минус прямая
/// родня. `csr_real`/`csr_permuted` — CSR ПРЯМОЙ со-встречаемости (то же,
/// что использовалось для G-A1′) — задаёт PPMI-рёбра реального и
/// переставленного графов.
#[allow(clippy::too_many_arguments)]
pub fn mine_via_walk_ppmi(
    store: &Store,
    csr_real: &Csr,
    csr_permuted: &Csr,
    frequency_floor: u64,
    ppmi_threshold: f64,
    seed_real: u64,
    seed_permuted: u64,
    seed_structural: u64,
) -> WalkPpmiOutcome {
    let corpus_valid_real = ppmi::valid_links(csr_real, frequency_floor, ppmi_threshold);
    let corpus_valid_permuted = ppmi::valid_links(csr_permuted, frequency_floor, ppmi_threshold);
    let corpus_valid_real_set: HashSet<(TokenId, TokenId)> =
        corpus_valid_real.iter().map(|e| normalize(e.token, e.context)).collect();
    let corpus_valid_permuted_set: HashSet<(TokenId, TokenId)> =
        corpus_valid_permuted.iter().map(|e| normalize(e.token, e.context)).collect();

    let graph_real = WalkGraph::build(store, &corpus_valid_real, 2);
    let graph_permuted = WalkGraph::build(store, &corpus_valid_permuted, 2);
    let graph_structural = WalkGraph::build(store, &[], 2);

    let walk_csr_real = collect_walk_coo(&graph_real, WALK_LENGTH, WALKS_PER_NODE, seed_real).bake_to_csr();
    let walk_csr_permuted = collect_walk_coo(&graph_permuted, WALK_LENGTH, WALKS_PER_NODE, seed_permuted).bake_to_csr();
    let walk_csr_structural =
        collect_walk_coo(&graph_structural, WALK_LENGTH, WALKS_PER_NODE, seed_structural).bake_to_csr();

    let walk_valid_real = normalized_valid_set(&walk_csr_real, frequency_floor, ppmi_threshold);
    let walk_valid_permuted = normalized_valid_set(&walk_csr_permuted, frequency_floor, ppmi_threshold);
    let background = normalized_valid_set(&walk_csr_structural, frequency_floor, ppmi_threshold);

    let filter = |walk_valid: &HashSet<(TokenId, TokenId)>, corpus_valid: &HashSet<(TokenId, TokenId)>| {
        let mut out: Vec<(TokenId, TokenId)> = walk_valid
            .iter()
            .filter(|p| !background.contains(p))
            .filter(|p| !corpus_valid.contains(p))
            .filter(|&&(a, b)| !graph_structural.is_structurally_connected(a, b))
            .filter(|&&(a, b)| !graph_structural.is_sibling_pair(a, b))
            .copied()
            .collect();
        out.sort_unstable();
        out
    };

    let candidates_real = filter(&walk_valid_real, &corpus_valid_real_set);
    let candidates_permuted = filter(&walk_valid_permuted, &corpus_valid_permuted_set);

    WalkPpmiOutcome {
        candidates_real,
        candidates_permuted,
        walk_valid_real: walk_valid_real.len(),
        walk_valid_permuted: walk_valid_permuted.len(),
        background,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn small_store_with_composition() -> (Store, TokenId, TokenId, TokenId) {
        let mut store = Store::new();
        store.init_factory();
        let a = store.intern(0, &[b'a' as u32]);
        let b = store.intern(0, &[b'b' as u32]);
        let ab = store.intern(1, &[a, b]);
        let c = store.intern(0, &[b'c' as u32]);
        let d = store.intern(0, &[b'd' as u32]);
        let cd = store.intern(1, &[c, d]);
        let abcd = store.intern(2, &[ab, cd]);
        (store, ab, cd, abcd)
    }

    #[test]
    fn structural_edges_are_bidirectional_and_level_filtered() {
        let (store, ab, cd, abcd) = small_store_with_composition();
        let graph = WalkGraph::build(&store, &[], 1);
        assert!(graph.is_structurally_connected(abcd, ab));
        assert!(graph.is_structurally_connected(ab, abcd), "рёбра двунаправленные");
        assert!(graph.is_structurally_connected(abcd, cd));
    }

    #[test]
    fn structural_edges_respect_min_level_band() {
        let (store, ab, cd, _abcd) = small_store_with_composition();
        let graph = WalkGraph::build(&store, &[], 2);
        assert!(!graph.nodes().contains(&ab));
        assert!(!graph.nodes().contains(&cd));
        assert!(graph.nodes().is_empty(), "abcd без рёбер в полосе >=2 -> не узел графа");
    }

    #[test]
    fn sibling_pairs_detected_regardless_of_band() {
        let (store, ab, cd, _abcd) = small_store_with_composition();
        let graph = WalkGraph::build(&store, &[], 0);
        assert!(graph.is_sibling_pair(ab, cd));
        assert!(graph.is_sibling_pair(cd, ab), "порядок не важен — нормализовано");
    }

    #[test]
    fn covisitation_excludes_distance_1_and_emits_both_directions() {
        let walk = vec![10u32, 20, 30, 40, 50];
        let got = covisitation_events(&walk, 2, 5);
        assert!(!got.iter().any(|&(a, b, _)| (a, b) == (10, 20) || (a, b) == (20, 10)), "дистанция 1 исключена");
        assert!(got.contains(&(10, 30, 1)));
        assert!(got.contains(&(30, 10, 1)), "оба направления эмитятся (обязательно для верных маргиналов PPMI)");
    }

    #[test]
    fn walk_stops_early_at_dead_end_not_panicking() {
        let mut store = Store::new();
        store.init_factory();
        let a = store.intern(0, &[b'a' as u32]);
        let b = store.intern(0, &[b'b' as u32]);
        let ab = store.intern(1, &[a, b]);
        let graph = WalkGraph::build(&store, &[], 1);
        let mut rng = SplitMix64::new(1);
        let walk = random_walk(&graph, ab, 40, &mut rng);
        assert_eq!(walk, vec![ab], "нет рёбер -> блуждание длины 1, не паника");
    }

    #[test]
    fn deterministic_walk_coo_for_fixed_seed() {
        let mut store = Store::new();
        store.init_factory();
        let a = store.intern(0, &[b'a' as u32]);
        let b = store.intern(0, &[b'b' as u32]);
        let ab = store.intern(1, &[a, b]);
        let c = store.intern(0, &[b'c' as u32]);
        let d = store.intern(0, &[b'd' as u32]);
        let cd = store.intern(1, &[c, d]);
        let abcd = store.intern(2, &[ab, cd]);
        let _ = abcd;

        let graph = WalkGraph::build(&store, &[], 1);
        let csr_a = collect_walk_coo(&graph, 10, 5, 0xBEEF).bake_to_csr();
        let csr_b = collect_walk_coo(&graph, 10, 5, 0xBEEF).bake_to_csr();
        assert_eq!(csr_a.row_ptr, csr_b.row_ptr);
        assert_eq!(csr_a.col_idx, csr_b.col_idx);
        assert_eq!(csr_a.values, csr_b.values);
    }

    #[test]
    fn structural_only_graph_has_no_cooccurrence_edges() {
        let (store, _ab, _cd, _abcd) = small_store_with_composition();
        let graph = WalkGraph::build(&store, &[], 1);
        for node in graph.nodes() {
            assert_eq!(graph.cooccurrence_degree(node), 0);
        }
    }

    #[test]
    fn pure_structural_walk_never_uses_cooccurrence_pick() {
        // Граф без PPMI-рёбер вовсе -> step() обязан ВСЕГДА идти
        // структурно (cooc пуст -> ветка (false,true) во всех случаях,
        // где structural непуст). Проверяем через детерминированный сид,
        // что блуждание остаётся в пределах структурно достижимого
        // множества узлов.
        let (store, ab, cd, abcd) = small_store_with_composition();
        let graph = WalkGraph::build(&store, &[], 1);
        let mut rng = SplitMix64::new(7);
        let walk = random_walk(&graph, abcd, 20, &mut rng);
        for &node in &walk {
            assert!(node == ab || node == cd || node == abcd, "блуждание вышло за пределы структурного компонента");
        }
    }

    #[test]
    fn candidates_exclude_background_corpus_known_and_kin() {
        // Простейший санитарный прогон: строим стор с composition-графом,
        // без PPMI-рёбер вообще (csr_real == csr_permuted == пустой) ->
        // все три прогона идентичны структурному фону -> кандидатов 0 в
        // обоих условиях (все walk-valid пары, если такие есть, попадают
        // в фон B и вычитаются).
        let (store, _ab, _cd, _abcd) = small_store_with_composition();
        let empty_coo = CooAccumulator::new();
        let empty_csr = empty_coo.bake_to_csr();

        let outcome = mine_via_walk_ppmi(&store, &empty_csr, &empty_csr, 3, 2.0, 1, 2, 3);
        assert!(
            outcome.candidates_real.is_empty(),
            "без PPMI-рёбер реальный граф == структурный -> фон вычитает все кандидаты"
        );
        assert!(outcome.candidates_permuted.is_empty());
    }
}
