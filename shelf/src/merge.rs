//! Шаг 2: SoA рабочей последовательности (Vec<u32> id + параллельный
//! Vec<u32> levels — не лезть в Store за уровнем на каждом шаге горячего
//! цикла).
//!
//! Шаг 3: счётчик пар на связном списке. При слиянии обновляются только
//! затронутые (не более 4) соседние позиции вместо полного пересчёта —
//! никакого повторного O(n)-пересканирования сырой последовательности на
//! каждое поколение (диагноз часа, блокнот 3, п.0).
//!
//! КАДАНС — поколениями, как в python-эталоне (не непрерывной глобальной
//! очередью): первая версия этого модуля брала на каждом шаге глобально
//! самую частую пару прямо сейчас, включая только что рождённые. На
//! игрушечных примерах расхождение с python было в пределах 15-20%, но на
//! реальном корпусе (bhagavad_gita.txt) вскрылось кратно: словарь в 3 раза
//! меньше (43 725 против 132 559 у python), held-out перенос просел с
//! 97/95% до 90/86%, форма распределения по уровням качественно другая
//! (плоский хвост до уровня 16 вместо горба на уровнях 3-4 и затухания к
//! уровню 12). Непрерывная очередь додавливает одну цепочку вглубь вместо
//! параллельного открытия словаря "поколением" — разный механизм роста, не
//! шум тай-брейка. Решение зафиксировано явно (не тихая замена, DEV_GUIDE
//! п.1): вернуть поколенческую границу, сохранив связный список вместо
//! O(n)-пересканирования текста на каждое поколение.
//!
//! Куда делась "бинарная куча" из буквы шага 3 плана: как только кандидаты
//! на поколение зафиксированы, порядок их обработки внутри поколения не
//! меняется до конца поколения (счётчики других кандидатов волны не
//! пересматриваются, ровно как у python) — единоразовая сортировка волны
//! даёт тот же результат, что и куча, без её сложности и без риска
//! повторить инвариантный баг из первой версии (устаревшая запись кучи,
//! отражающая счёт, который никогда явно не пушился). Явное упрощение,
//! не молчаливая просадка требования.
//!
//! НАЙДЕННЫЙ И ИСПРАВЛЕННЫЙ БАГ (не архитектурное расхождение): после
//! перехода на поколенческий каданс словарь на bhagavad_gita.txt всё
//! равно был меньше python на треть (83 536 против 132 559). Причина —
//! `store.intern()` вызывался ВНУТРИ цикла по позициям кандидата, после
//! проверки `alive`, то есть только если у кандидата оставалось хоть одно
//! валидное вхождение к моменту обработки. python вызывает intern()
//! БЕЗУСЛОВНО один раз на кандидата, до перебора его позиций — рождение
//! токена не зависит от того, сколько вхождений реально survives
//! перекрытие с более ранними кандидатами той же волны. Перенос intern()
//! наружу цикла по позициям дал побитовое совпадение с python при
//! ContentHash-тайбрейке (словарь 132 559 = 132 559, длина после свёртки
//! 202 159 = 202 159, births по поколениям идентичны). При принятом
//! BirthOrder-тайбрейке расхождение сократилось до ~0.2% (132 360 против
//! 132 559) — это и есть тот самый symmetric noise тай-брейка, который
//! ожидался с самого начала, а не систематический сдвиг.

use crate::store::{Store, TokenId};
use std::collections::{BTreeSet, HashMap};

pub struct Sequence {
    pub ids: Vec<TokenId>,
    pub levels: Vec<u32>,
}

impl Sequence {
    pub fn from_ids(store: &Store, ids: Vec<TokenId>) -> Self {
        let levels = ids.iter().map(|&id| store.level_of(id)).collect();
        Sequence { ids, levels }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

type Pair = (TokenId, TokenId);

/// Предохранитель на число поколений — то же значение (K=200), что и в
/// принятом python-эталоне (arch/этап 1/axmv2.py). Каскад сходится сам
/// (см. STATUS.md), это именно предохранитель, не рабочий механизм
/// остановки.
pub const MAX_GENERATIONS: usize = 200;

fn remove_occurrence(occurrences: &mut HashMap<Pair, BTreeSet<usize>>, pair: Pair, pos: usize) {
    if let Some(set) = occurrences.get_mut(&pair) {
        set.remove(&pos);
    }
}

fn insert_occurrence(occurrences: &mut HashMap<Pair, BTreeSet<usize>>, pair: Pair, pos: usize) {
    occurrences.entry(pair).or_default().insert(pos);
}

/// Как сортировать кандидатов волны при равной частоте.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TieBreak {
    /// Принятый по умолчанию выбор шага 3: порядок рождения токена
    /// (u32-индекс интернирования). НЕ нейтральный шум — систематически
    /// предпочитает раньше открытые токены при равной частоте пары.
    BirthOrder,
    /// Диагностика: порядок по байтам content-хеша (blake2b-16, тот же
    /// алгоритм, что python-эталон), как в python. SHA-256-идентичность
    /// токена (шаг 1) не меняется — это только компаратор сортировки
    /// волны, не хранимая структура.
    ContentHash,
}

fn alive_snapshot(ids: &[TokenId], alive: &[bool]) -> Vec<TokenId> {
    (0..ids.len()).filter(|&i| alive[i]).map(|i| ids[i]).collect()
}

/// Прогнать библиотекаря до сходимости поколениями (ни одно поколение
/// больше не рождает токенов — births == 0, как у python) либо до
/// MAX_GENERATIONS. Возвращает свёрнутую последовательность.
pub fn run(store: &mut Store, seq: Sequence, threshold: u64, tie_break: TieBreak) -> Sequence {
    run_traced(store, seq, threshold, tie_break).0
}

/// Как `run`, но дополнительно возвращает births по каждому поколению —
/// прямой аналог python `births_per_iteration`, для диагностической
/// сверки поколение-к-поколению с эталонным снапшотом.
pub fn run_traced(
    store: &mut Store,
    seq: Sequence,
    threshold: u64,
    tie_break: TieBreak,
) -> (Sequence, Vec<usize>) {
    run_with_generation_hook(store, seq, threshold, tie_break, |_, _| {})
}

/// Как `run_traced`, но `hook` вызывается в начале КАЖДОГО поколения
/// (после проверки "волна пуста -> стоп", до сортировки волны и любых
/// слияний этого поколения) с текущим живым срезом последовательности —
/// "легальный текст" над словарём на этот момент (план 3, Правка 1,
/// Р-А4-1). Сам алгоритм слияния hook не видит и не может изменить —
/// это только точка наблюдения, не точка вмешательства. Используется для
/// по-поколенной аккумуляции контекста перевёрнутой библиотеки (шаг 8);
/// `run`/`run_traced` — то же самое с hook-пустышкой, поведение и
/// побитовое совпадение с python-эталоном не меняются.
pub fn run_with_generation_hook<F: FnMut(&Store, &[TokenId])>(
    store: &mut Store,
    seq: Sequence,
    threshold: u64,
    tie_break: TieBreak,
    mut hook: F,
) -> (Sequence, Vec<usize>) {
    let mut births_per_generation = Vec::new();
    let n = seq.len();
    if n < 2 {
        return (seq, births_per_generation);
    }
    let threshold = threshold as usize;

    let mut ids = seq.ids;
    let mut levels = seq.levels;
    let mut prev: Vec<i64> = (0..n as i64).map(|i| i - 1).collect();
    let mut next: Vec<i64> = (0..n as i64)
        .map(|i| if i + 1 < n as i64 { i + 1 } else { -1 })
        .collect();
    let mut alive = vec![true; n];

    let mut occurrences: HashMap<Pair, BTreeSet<usize>> = HashMap::new();
    for i in 0..n - 1 {
        insert_occurrence(&mut occurrences, (ids[i], ids[i + 1]), i);
    }

    for _generation in 0..MAX_GENERATIONS {
        // Волна: снимок кандидатов НА МОМЕНТ старта поколения — тот же
        // тайбрейк, что у python — (-count, a, b) по возрастанию, — И,
        // критично, снимок самих ПОЗИЦИЙ (`Vec<usize>`, а не живая ссылка
        // на occurrences[pair]). Раньше вхождения читались из occurrences
        // лениво, в момент обработки кандидата — значит, если более ранний
        // кандидат ЭТОЙ ЖЕ волны своим слиянием случайно порождал НОВОЕ
        // вхождение уже запланированного кандидата, оно тут же сливалось
        // в этом же поколении. python так не делает: `pair_positions`
        // строится один раз в начале итерации и замораживается —
        // новорождённые внутри итерации вхождения ждут следующей. На
        // реальном корпусе (bhagavad_gita.txt) это не влияло на длину
        // итоговой последовательности (обе версии сходились к одному
        // числу живых позиций), но раздувало число поколений, за которые
        // каждый уровень открывается, и оказалось источником словарного
        // разрыва с python — исправлено здесь.
        let mut wave: Vec<(Pair, Vec<usize>)> = occurrences
            .iter()
            .filter(|(_, positions)| positions.len() >= threshold)
            .map(|(&pair, positions)| (pair, positions.iter().copied().collect()))
            .collect();
        if wave.is_empty() {
            break;
        }

        // Точка наблюдения (не вмешательства) для по-поколенной
        // аккумуляции контекста (план 3, Правка 1): срез — живая
        // последовательность НА ВХОДЕ в это поколение, до его слияний.
        let snapshot = alive_snapshot(&ids, &alive);
        hook(store, &snapshot);

        wave.sort_by(|(pa, posa), (pb, posb)| {
            posb.len().cmp(&posa.len()).then_with(|| match tie_break {
                TieBreak::BirthOrder => pa.cmp(pb),
                TieBreak::ContentHash => {
                    let ha = (store.tie_hash_of(pa.0), store.tie_hash_of(pa.1));
                    let hb = (store.tie_hash_of(pb.0), store.tie_hash_of(pb.1));
                    ha.cmp(&hb)
                }
            })
        });

        let mut births = 0usize;

        for (pair, frozen_positions) in wave {
            // Как у python: intern() кандидата вызывается БЕЗУСЛОВНО один
            // раз, до перебора его позиций — рождение токена не зависит от
            // того, останется ли у него хоть одно валидное вхождение после
            // того, как более ранние кандидаты этой же волны (перекрытие
            // позиций) съели часть соседей. Раньше intern() стоял ВНУТРИ
            // цикла по позициям, за проверкой alive — если все вхождения
            // кандидата оказывались инвалидированы, токен вообще не
            // рождался. Это и было источником разрыва словаря с python:
            // на реальном корпусе (bhagavad_gita.txt, поколение 0) 880
            // байт-пар корректно проходили порог, но только 413 из них
            // сохраняли хоть одно валидное вхождение к моменту обработки
            // — остальные 467 python всё равно интернирует (пустышкой,
            // count=0 в этом поколении, но токен существует и попадёт в
            // счёт словаря), а старый код Rust — нет.
            let new_level = store.level_of(pair.0).max(store.level_of(pair.1)) + 1;
            let before = store.birth_log().len();
            let new_id = store.intern(new_level, &[pair.0, pair.1]);
            if store.birth_log().len() > before {
                births += 1;
            }

            // Как у python: `alive[i] and alive[i+1] and seq[i]==a and
            // seq[i+1]==b` — вхождение из замороженного снимка могло
            // перестать быть валидным из-за более раннего кандидата ЭТОЙ
            // ЖЕ волны (перекрытие позиций), тогда просто пропускаем его.
            for pos in frozen_positions {
                if !alive[pos] {
                    continue;
                }
                let j = next[pos];
                if j == -1 || !alive[j as usize] {
                    continue;
                }
                let j = j as usize;
                if ids[pos] != pair.0 || ids[j] != pair.1 {
                    continue;
                }

                remove_occurrence(&mut occurrences, pair, pos);

                let p = prev[pos];
                let q = next[j];

                store.touch(new_id);

                if p != -1 {
                    let p = p as usize;
                    let old_left = (ids[p], pair.0);
                    remove_occurrence(&mut occurrences, old_left, p);
                    insert_occurrence(&mut occurrences, (ids[p], new_id), p);
                }

                if q != -1 {
                    let qu = q as usize;
                    let old_right = (pair.1, ids[qu]);
                    remove_occurrence(&mut occurrences, old_right, j);
                    insert_occurrence(&mut occurrences, (new_id, ids[qu]), pos);
                }

                ids[pos] = new_id;
                levels[pos] = new_level;
                alive[j] = false;
                next[pos] = q;
                if q != -1 {
                    prev[q as usize] = pos as i64;
                }
            }
        }

        births_per_generation.push(births);
        if births == 0 {
            break;
        }
    }

    let out_ids = alive_snapshot(&ids, &alive);
    let out_levels: Vec<u32> = (0..n).filter(|&i| alive[i]).map(|i| levels[i]).collect();
    (
        Sequence {
            ids: out_ids,
            levels: out_levels,
        },
        births_per_generation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intern_bytes(store: &mut Store, bytes: &[u8]) -> Vec<TokenId> {
        bytes.iter().map(|&b| store.intern(0, &[b as u32])).collect()
    }

    fn rendered(store: &Store, seq: &Sequence) -> Vec<Vec<u8>> {
        seq.ids.iter().map(|&id| store.bytes_of(id)).collect()
    }

    #[test]
    fn non_overlapping_pairs_merge_like_naive_reference() {
        let mut store = Store::new();
        store.init_factory();
        let ids = intern_bytes(&mut store, b"abab");
        let seq = Sequence::from_ids(&store, ids);
        let out = run(&mut store, seq, 2, TieBreak::BirthOrder);
        assert_eq!(rendered(&store, &out), vec![b"ab".to_vec(), b"ab".to_vec()]);
    }

    #[test]
    fn cascading_merge_builds_higher_level_token_over_generations() {
        let mut store = Store::new();
        store.init_factory();
        let ids = intern_bytes(&mut store, b"abababab");
        let seq = Sequence::from_ids(&store, ids);
        let out = run(&mut store, seq, 2, TieBreak::BirthOrder);
        assert_eq!(out.len(), 2);
        for &id in &out.ids {
            assert_eq!(store.bytes_of(id), b"abab");
            assert!(store.level_of(id) >= 2);
        }
    }

    #[test]
    fn below_threshold_pair_is_never_merged() {
        let mut store = Store::new();
        store.init_factory();
        let ids = intern_bytes(&mut store, b"ab");
        let seq = Sequence::from_ids(&store, ids);
        let out = run(&mut store, seq, 2, TieBreak::BirthOrder);
        assert_eq!(out.len(), 2);
        assert_eq!(store.level_of(out.ids[0]), 0);
        assert_eq!(store.level_of(out.ids[1]), 0);
    }

    #[test]
    fn triple_repeat_documents_left_greedy_overlap_choice() {
        let mut store = Store::new();
        store.init_factory();
        let ids = intern_bytes(&mut store, b"aaa");
        let seq = Sequence::from_ids(&store, ids);
        let out = run(&mut store, seq, 2, TieBreak::BirthOrder);
        assert_eq!(out.len(), 2);
        assert_eq!(store.bytes_of(out.ids[0]), b"aa");
        assert_eq!(store.level_of(out.ids[0]), 1);
        assert_eq!(store.bytes_of(out.ids[1]), b"a");
        assert_eq!(store.level_of(out.ids[1]), 0);
    }

    #[test]
    fn dedup_same_composition_across_positions_yields_same_id() {
        let mut store = Store::new();
        store.init_factory();
        let ids = intern_bytes(&mut store, b"xy xy");
        let seq = Sequence::from_ids(&store, ids);
        let out = run(&mut store, seq, 2, TieBreak::BirthOrder);
        let xy_positions: Vec<TokenId> = out
            .ids
            .iter()
            .copied()
            .filter(|&id| store.bytes_of(id) == b"xy")
            .collect();
        assert_eq!(xy_positions.len(), 2);
        assert_eq!(xy_positions[0], xy_positions[1]);
    }

    #[test]
    fn new_pair_born_this_generation_waits_for_the_next() {
        // "aaaaaaaa" (8 x 'a'), T=2: поколение 1 сливает (a,a) жадно
        // слева направо в 4x"aa" (level1); пара ("aa","aa") рождена ТОЛЬКО
        // что этим поколением и в него не входит (её не было в снимке
        // волны) — ждёт поколения 2, где её текущий счёт (3) >= T, и она
        // сливается в 2x"aaaa" (level2). Пара ("aaaa","aaaa") рождается
        // поколением 2 и в него не входит; в поколении 3 её счёт — 1
        // (всего одна пара соседних "aaaa" на 2 токена), это < T=2, и
        // финального слияния в один токен "aaaaaaaa" не происходит.
        // Если бы поколения не разделялись (старая непрерывная версия),
        // каскад продавил бы всё за один run() до конца.
        let mut store = Store::new();
        store.init_factory();
        let ids = intern_bytes(&mut store, b"aaaaaaaa"); // 8 x 'a'
        let seq = Sequence::from_ids(&store, ids);
        let out = run(&mut store, seq, 2, TieBreak::BirthOrder);
        assert_eq!(out.len(), 2);
        for &id in &out.ids {
            assert_eq!(store.bytes_of(id), b"aaaa");
            assert_eq!(store.level_of(id), 2);
        }
    }

    #[test]
    fn generation_hook_fires_once_per_converged_generation_with_correct_snapshot() {
        // "aaaaaaaa" сходится ровно за 2 поколения (см. комментарий
        // above): поколение 1 видит "aaaaaaaa" (8xa), поколение 2 видит
        // "aaaaaaaa" as 4x"aa". Поколение 3 не запускается (its wave
        // была бы пуста — счёт пары "aaaa","aaaa" = 1 < T=2), значит хук
        // обязан выстрелить ровно 2 раза, не 3.
        let mut store = Store::new();
        store.init_factory();
        let ids = intern_bytes(&mut store, b"aaaaaaaa");
        let seq = Sequence::from_ids(&store, ids);

        let mut snapshots: Vec<Vec<u8>> = Vec::new();
        let (_out, births) = run_with_generation_hook(&mut store, seq, 2, TieBreak::BirthOrder, |s, sequence| {
            let rendered: Vec<u8> = sequence.iter().flat_map(|&id| s.bytes_of(id)).collect();
            snapshots.push(rendered);
        });

        assert_eq!(births.len(), 2, "два поколения дают births, третье не запускается вовсе");
        assert_eq!(snapshots.len(), 2, "хук обязан выстрелить ровно на сошедшиеся поколения");
        assert_eq!(snapshots[0], b"aaaaaaaa", "поколение 1 видит исходный вход целиком");
        assert_eq!(snapshots[1], b"aaaaaaaa", "поколение 2 видит тот же текст, уже как 4 токена 'aa'");
    }

    #[test]
    fn hook_does_not_change_algorithm_behaviour() {
        // run/run_traced с hook-пустышкой обязаны давать тот же результат,
        // что и прогон с непустым hook — hook только читает, не влияет.
        let mut store_a = Store::new();
        store_a.init_factory();
        let ids_a = intern_bytes(&mut store_a, b"the cat sat on the mat, the cat sat again");
        let seq_a = Sequence::from_ids(&store_a, ids_a);
        let out_a = run(&mut store_a, seq_a, 2, TieBreak::BirthOrder);

        let mut store_b = Store::new();
        store_b.init_factory();
        let ids_b = intern_bytes(&mut store_b, b"the cat sat on the mat, the cat sat again");
        let seq_b = Sequence::from_ids(&store_b, ids_b);
        let mut hook_calls = 0usize;
        let (out_b, _) = run_with_generation_hook(&mut store_b, seq_b, 2, TieBreak::BirthOrder, |_, _| {
            hook_calls += 1;
        });

        assert!(hook_calls > 0);
        assert_eq!(out_a.ids, out_b.ids);
        assert_eq!(out_a.levels, out_b.levels);
    }
}
