//! Шаг 12 (план 3): смерть как generational GC — mark без sweep.
//!
//! **Смерть — не мутация поля, а запись в append-only логе.** Токен не
//! удаляется и не помечается флагом на своём теле (тело в `Store`
//! неизменно, INVARIANTS.md) — вместо этого лог фокуса накапливает
//! события `Unfocus{token, revolution}` / `Focus{token, revolution}`.
//! Воскрешение — просто более поздняя запись `Focus`, не отдельный
//! механизм: "смерть — прекращение обращения, не удаление" здесь
//! буквально тип записи, не метафора.
//!
//! **Корневое множество** (закрывает открытый вопрос §11.2 блокнота 4,
//! план 3 "принято"): корень = токен, входящий в свёртку потока
//! (`WheelState::last_touched_revolution`) за последние `ROOT_WINDOW_
//! REVOLUTIONS` (K=3) оборотов, плюс фабричное дно (уровень 0,
//! бессмертно по инварианту — держит идентичность байтов, из которых
//! всё остальное состоит). Достижимость — вниз по составу от корней
//! (дети, внуки, ... до байтов): узел, достижимый через живого родителя,
//! сам жив, даже если давно не встречался напрямую.
//!
//! **Срез по весу (Правка 6, план 3, 2026-07-18):** число получено —
//! квантиль 25% пограничных, тай-брейк одноконтекстными. "Пограничные" =
//! достижимые, но не сами корни (`reachable \ roots`) — живые только
//! через чужую композицию, не по собственному недавнему касанию. Вес =
//! накопленный PPMI-вес живых связей токена (сумма PPMI по валидным
//! связям перевёрнутой полки, где ОБА конца ещё достижимы — связь с уже
//! мёртвым не в счёт). Нижний квартиль по весу среди пограничных уходит
//! в Unfocus; при равном весе на границе квартиля раньше уходят
//! одноконтекстные (число валидных связей < 2, приближение "контекстного
//! разнообразия" через степень в PPMI-графе перевёрнутой полки —
//! кандидат 1 блокнота 4). См. `weight_cut`.
//!
//! **"Три исхода из одной операции" (кандидат 6, Арканы) в буквальном
//! виде** — это механика конкурирующих пар из рефлексии, не переведённая
//! планом в инженерные термины отдельным числом. Достижимость от
//! K=3-оборотных корней — самостоятельный mark-механизм с собственными
//! тремя естественными исходами на одном `mark()` (см. `MarkOutcome`),
//! не та же механика, что в Арканах; `weight_cut` — отдельный, второй
//! проход среза поверх уже размеченного достижимого множества.
//!
//! Достижимость от корней — то, что план явно принял числом (K=3) и
//! механизмом (вниз по составу) — реализована здесь полностью и
//! персистентна с той же дисциплиной replay-верификации, что шаг 6.

use crate::context::{filter_by_min_level, ContextExtractor, Window};
use crate::cooc::CooAccumulator;
use crate::ppmi::{self, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD};
use crate::store::{Store, TokenId};
use crate::wheel::WheelState;
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

/// K из плана 3 ("Корневое множество... K=3 оборота... принято").
pub const ROOT_WINDOW_REVOLUTIONS: u32 = 3;
/// Правка 6: доля пограничных, срезаемых по весу за один проход.
pub const WEIGHT_CUT_QUANTILE: f64 = 0.25;
/// Приближение "контекстного разнообразия" (кандидат 1 блокнота 4): число
/// валидных PPMI-связей токена на перевёрнутой полке. Меньше этого —
/// "одноконтекстный", приоритет на выход при равном весе.
pub const ONE_CONTEXT_DIVERSITY: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusEvent {
    Focus { token: TokenId, revolution: u32 },
    Unfocus { token: TokenId, revolution: u32 },
}

/// Три естественных исхода одного вызова `DeathLog::mark` — не механика
/// конкурирующих пар из Арканов (см. шапку модуля), а прямое следствие
/// сравнения "было ли достижимо / стало ли достижимо".
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MarkOutcome {
    pub newly_focused: usize,
    pub newly_unfocused: usize,
    pub unchanged: usize,
}

/// Результат одного вызова `DeathLog::weight_cut` (Правка 6).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WeightCutOutcome {
    pub borderline_count: usize,
    pub cut_count: usize,
}

/// Концепт, находящийся в активном фокусе ядра памяти.
#[derive(Debug, Clone, PartialEq)]
pub struct FocusConcept {
    pub token_id: TokenId,
    pub text: String,
    pub level: u8,
    pub ppmi_weight: f64,
}

/// Неизменяемый снимок активного фокуса для неблокирующего (lock-free) чтения.
#[derive(Debug, Clone, PartialEq)]
pub struct FocusSet {
    pub revolution: u32,
    pub tokens: HashSet<TokenId>,
    pub top_concepts: Vec<FocusConcept>,
}

impl FocusSet {
    pub fn new(revolution: u32, tokens: HashSet<TokenId>, top_concepts: Vec<FocusConcept>) -> Self {
        Self {
            revolution,
            tokens,
            top_concepts,
        }
    }

    pub fn empty() -> Self {
        Self {
            revolution: 0,
            tokens: HashSet::new(),
            top_concepts: Vec::new(),
        }
    }

    #[inline]
    pub fn contains(&self, id: TokenId) -> bool {
        self.tokens.contains(&id)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    pub fn tokens(&self) -> &HashSet<TokenId> {
        &self.tokens
    }

    pub fn top_concepts(&self) -> &[FocusConcept] {
        &self.top_concepts
    }

    pub fn revolution(&self) -> u32 {
        self.revolution
    }
}

/// Корневое множество на текущий момент Колеса: фабричное дно (уровень 0,
/// бессмертно) + токены, входившие в свёртку потока за последние K
/// оборотов.
pub fn root_set(wheel: &WheelState) -> HashSet<TokenId> {
    let mut roots = HashSet::with_capacity(256 + wheel.last_touched_revolution.len());
    let factory_count = 256.min(wheel.store.len() as TokenId);
    for id in 0..factory_count {
        if wheel.store.level_of(id) == 0 {
            roots.insert(id);
        }
    }
    for (&id, &last) in &wheel.last_touched_revolution {
        if wheel.revolution.saturating_sub(last) < ROOT_WINDOW_REVOLUTIONS {
            roots.insert(id);
        }
    }
    roots
}

/// Достижимость вниз по составу от корней (дети, внуки, ... до байтов).
pub fn reachable_set(store: &Store, roots: &HashSet<TokenId>) -> HashSet<TokenId> {
    let mut reachable: HashSet<TokenId> = HashSet::new();
    let mut stack: Vec<TokenId> = roots.iter().copied().collect();
    while let Some(id) = stack.pop() {
        if !reachable.insert(id) {
            continue;
        }
        let (level, children) = store.get(id);
        if level > 0 {
            for &c in children {
                if !reachable.contains(&c) {
                    stack.push(c);
                }
            }
        }
    }
    reachable
}

/// Append-only лог событий фокуса + текущее восстановимое состояние
/// (какие токены сейчас достижимы) — тот же разрез "тело/вид", что у
/// Store: `events` — история, `focused` — вид, выводимый из истории
/// проходом (см. `replay`).
#[derive(Default)]
pub struct DeathLog {
    events: Vec<FocusEvent>,
    focused: HashSet<TokenId>,
}

impl DeathLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_parts(events: Vec<FocusEvent>, focused: HashSet<TokenId>) -> Self {
        Self { events, focused }
    }

    /// Пересчитать достижимость для текущего состояния Колеса и
    /// записать переходы как события. Порядок обхода токенов —
    /// возрастающий TokenId (детерминизм, Р2): порядок событий одного
    /// mark() воспроизводим независимо от порядка HashSet-обхода.
    pub fn mark(&mut self, wheel: &WheelState) -> MarkOutcome {
        let roots = root_set(wheel);
        let reachable = reachable_set(&wheel.store, &roots);
        let mut outcome = MarkOutcome::default();

        let mut newly_focused: Vec<TokenId> = reachable
            .iter()
            .copied()
            .filter(|id| !self.focused.contains(id))
            .collect();
        let mut newly_unfocused: Vec<TokenId> = self
            .focused
            .iter()
            .copied()
            .filter(|id| !reachable.contains(id))
            .collect();

        outcome.newly_focused = newly_focused.len();
        outcome.newly_unfocused = newly_unfocused.len();
        outcome.unchanged = wheel.store.len().saturating_sub(outcome.newly_focused + outcome.newly_unfocused);

        newly_focused.sort_unstable();
        newly_unfocused.sort_unstable();

        let mut i = 0;
        let mut j = 0;
        while i < newly_focused.len() && j < newly_unfocused.len() {
            if newly_focused[i] < newly_unfocused[j] {
                let id = newly_focused[i];
                self.events.push(FocusEvent::Focus { token: id, revolution: wheel.revolution });
                self.focused.insert(id);
                i += 1;
            } else {
                let id = newly_unfocused[j];
                self.events.push(FocusEvent::Unfocus { token: id, revolution: wheel.revolution });
                self.focused.remove(&id);
                j += 1;
            }
        }
        while i < newly_focused.len() {
            let id = newly_focused[i];
            self.events.push(FocusEvent::Focus { token: id, revolution: wheel.revolution });
            self.focused.insert(id);
            i += 1;
        }
        while j < newly_unfocused.len() {
            let id = newly_unfocused[j];
            self.events.push(FocusEvent::Unfocus { token: id, revolution: wheel.revolution });
            self.focused.remove(&id);
            j += 1;
        }

        outcome
    }

    /// Создает неизменяемый снимок активного фокуса (`FocusSet`) для неблокирующего чтения MCP-ресурсами.
    /// Извлекает топ-концепты (составные токены с level >= 1), упорядоченные по силе ассоциаций.
    pub fn snapshot(&self, wheel: &WheelState, max_concepts: usize) -> FocusSet {
        let mut top_concepts = Vec::new();
        if max_concepts > 0 && !wheel.sequence.ids.is_empty() {
            let window = Window::range(2, 5);
            let events = window.extract(&wheel.store, &wheel.sequence.ids);
            let filtered = filter_by_min_level(&wheel.store, events, 1);
            let mut coo = CooAccumulator::new();
            coo.accumulate(&filtered);
            let csr = coo.bake_to_csr();
            let valid = ppmi::valid_links(&csr, 1, 0.0);

            let mut weights: HashMap<TokenId, f64> = HashMap::new();
            for link in &valid {
                if self.focused.contains(&link.token) && self.focused.contains(&link.context) {
                    *weights.entry(link.token).or_insert(0.0) += link.ppmi;
                }
            }

            let mut candidate_ids: Vec<TokenId> = self
                .focused
                .iter()
                .copied()
                .filter(|&id| wheel.store.level_of(id) >= 1)
                .collect();

            // Сортировка: по PPMI-весу убывающе, затем по уровню убывающе, затем по TokenId возрастающе
            candidate_ids.sort_by(|&a, &b| {
                let wa = weights.get(&a).copied().unwrap_or(0.0);
                let wb = weights.get(&b).copied().unwrap_or(0.0);
                wb.partial_cmp(&wa)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| wheel.store.level_of(b).cmp(&wheel.store.level_of(a)))
                    .then_with(|| a.cmp(&b))
            });

            for &id in candidate_ids.iter().take(max_concepts) {
                let raw_bytes = wheel.store.bytes_of(id);
                let text = String::from_utf8_lossy(&raw_bytes).to_string();
                let ppmi_weight = weights.get(&id).copied().unwrap_or(0.0);
                let level = wheel.store.level_of(id) as u8;
                top_concepts.push(FocusConcept {
                    token_id: id,
                    text,
                    level,
                    ppmi_weight,
                });
            }
        }

        FocusSet {
            revolution: wheel.revolution,
            tokens: self.focused.clone(),
            top_concepts,
        }
    }

    /// Правка 6 (план 3): срез по весу среди ПОГРАНИЧНЫХ (достижимых, но
    /// не корней напрямую) — квантиль 25% с наименьшим накопленным
    /// PPMI-весом живых связей уходит в Unfocus. Тай-брейк при равном
    /// весе — одноконтекстные (< `ONE_CONTEXT_DIVERSITY` валидных связей)
    /// раньше в очереди на срез. Требует, чтобы `mark()` уже был вызван
    /// для этого состояния Колеса (срез работает над уже размеченным
    /// достижимым множеством, не пересчитывает его заново).
    pub fn weight_cut(&mut self, wheel: &WheelState) -> WeightCutOutcome {
        let roots = root_set(wheel);
        let reachable = reachable_set(&wheel.store, &roots);
        let borderline: Vec<TokenId> = reachable.iter().copied().filter(|id| !roots.contains(id)).collect();

        // Вес = накопленный PPMI-вес живых связей (перевёрнутая полка,
        // окно 2..5, тот же PPMI-модуль, что судит поток в шагах 8-11).
        let window = Window::range(2, 5);
        let events = window.extract(&wheel.store, &wheel.sequence.ids);
        let filtered = filter_by_min_level(&wheel.store, events, 2);
        let mut coo = CooAccumulator::new();
        coo.accumulate(&filtered);
        let csr = coo.bake_to_csr();
        let valid = ppmi::valid_links(&csr, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD);

        let mut weight: HashMap<TokenId, f64> = HashMap::new();
        let mut diversity: HashMap<TokenId, usize> = HashMap::new();
        for entry in &valid {
            // "живая связь" — оба конца ещё достижимы; связь с уже мёртвым не в счёт.
            if reachable.contains(&entry.token) && reachable.contains(&entry.context) {
                *weight.entry(entry.token).or_insert(0.0) += entry.ppmi;
                *diversity.entry(entry.token).or_insert(0) += 1;
            }
        }

        let mut scored: Vec<(TokenId, f64, usize)> = borderline
            .iter()
            .map(|&id| {
                (
                    id,
                    weight.get(&id).copied().unwrap_or(0.0),
                    diversity.get(&id).copied().unwrap_or(0),
                )
            })
            .collect();
        // Возрастание по весу; при равном весе одноконтекстные (меньшее
        // разнообразие) раньше — тай-брейк Правки 6. TokenId последним
        // членом сортировки — детерминизм при полных совпадениях (Р2).
        scored.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap()
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.0.cmp(&b.0))
        });

        let cut_count = ((scored.len() as f64) * WEIGHT_CUT_QUANTILE).round() as usize;
        let mut newly_unfocused = 0usize;
        for &(id, _, _) in scored.iter().take(cut_count) {
            if self.focused.contains(&id) {
                self.events.push(FocusEvent::Unfocus { token: id, revolution: wheel.revolution });
                self.focused.remove(&id);
                newly_unfocused += 1;
            }
        }

        WeightCutOutcome {
            borderline_count: borderline.len(),
            cut_count: newly_unfocused,
        }
    }

    pub fn events(&self) -> &[FocusEvent] {
        &self.events
    }

    pub fn is_focused(&self, token: TokenId) -> bool {
        self.focused.contains(&token)
    }

    /// Внести токен в фокус вручную (например, при приёме через Порт)
    pub fn focus(&mut self, token: TokenId, revolution: u64) {
        if !self.focused.contains(&token) {
            self.events.push(FocusEvent::Focus { token, revolution: revolution as u32 });
            self.focused.insert(token);
        }
    }


    pub fn focused_set(&self) -> &HashSet<TokenId> {
        &self.focused
    }


    /// Восстановить состояние фокуса проходом по логу — та же дисциплина
    /// replay-верификации, что у persist.rs (шаг 6): вид выводится из
    /// истории, не хранится отдельно от неё как источник истины.
    pub fn replay(events: &[FocusEvent]) -> HashSet<TokenId> {
        let mut focused = HashSet::new();
        for &event in events {
            match event {
                FocusEvent::Focus { token, .. } => {
                    focused.insert(token);
                }
                FocusEvent::Unfocus { token, .. } => {
                    focused.remove(&token);
                }
            }
        }
        focused
    }
}

pub(crate) const DEATH_LOG_TABLE: TableDefinition<u32, (u8, u32, u32)> = TableDefinition::new("death_log");

#[derive(Debug)]
pub enum DeathLoadError {
    Redb(redb::Error),
}

impl fmt::Display for DeathLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeathLoadError::Redb(e) => write!(f, "redb: {e}"),
        }
    }
}

impl std::error::Error for DeathLoadError {}

impl From<redb::Error> for DeathLoadError {
    fn from(e: redb::Error) -> Self {
        DeathLoadError::Redb(e)
    }
}

pub(crate) fn encode_event(event: FocusEvent) -> (u8, u32, u32) {
    match event {
        FocusEvent::Focus { token, revolution } => (0, token, revolution),
        FocusEvent::Unfocus { token, revolution } => (1, token, revolution),
    }
}

pub(crate) fn decode_event(raw: (u8, u32, u32)) -> FocusEvent {
    let (kind, token, revolution) = raw;
    match kind {
        0 => FocusEvent::Focus { token, revolution },
        1 => FocusEvent::Unfocus { token, revolution },
        other => panic!("неизвестный тип записи лога фокуса: {other} (повреждённый файл?)"),
    }
}

/// Дописать в лог события, случившиеся после последнего сохранения —
/// тот же инкрементальный append-only паттерн, что `persist::save`
/// (шаг 6): длина таблицы == число уже записанных событий, дозаписывается
/// только хвост, одна redb-транзакция на вызов.
pub fn save(log: &DeathLog, path: &Path) -> Result<(), redb::Error> {
    let db = Database::create(path)?;
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(DEATH_LOG_TABLE)?;
        let already_persisted = table.len()? as u32;
        for (i, &event) in log.events.iter().enumerate().skip(already_persisted as usize) {
            table.insert(i as u32, encode_event(event))?;
        }
    }
    write_txn.commit()?;
    Ok(())
}

/// Восстановить лог проходом — воспроизводит `focused` тем же путём, что
/// `DeathLog::mark` изначально его строил (через `replay`, не отдельную
/// сериализацию состояния).
pub fn load(path: &Path) -> Result<DeathLog, DeathLoadError> {
    let db = Database::open(path).map_err(redb::Error::from)?;
    let read_txn = db.begin_read().map_err(redb::Error::from)?;
    let table = read_txn.open_table(DEATH_LOG_TABLE).map_err(redb::Error::from)?;

    let mut events = Vec::new();
    for entry in table.iter().map_err(redb::Error::from)? {
        let (_key, value) = entry.map_err(redb::Error::from)?;
        events.push(decode_event(value.value()));
    }

    let focused = DeathLog::replay(&events);
    Ok(DeathLog { events, focused })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::TieBreak;

    #[test]
    fn factory_tokens_are_always_roots_and_reachable() {
        let wheel = WheelState::new();
        let roots = root_set(&wheel);
        for b in 0..256u32 {
            assert!(roots.contains(&b), "байт {b} обязан быть корнем — фабричное дно бессмертно");
        }
    }

    #[test]
    fn token_outside_root_window_is_unfocused_but_reachable_child_stays_focused() {
        let mut wheel = WheelState::new();
        // Оборот 1: "ab" появляется и уходит из окна корней после этого.
        wheel.turn(b"ab", 2, TieBreak::BirthOrder);
        let a = wheel.store.intern(0, &[b'a' as u32]);
        let b = wheel.store.intern(0, &[b'b' as u32]);
        let ab = wheel.store.intern(1, &[a, b]);
        // Заставим "ab" реально устареть: несколько оборотов без него.
        for _ in 0..ROOT_WINDOW_REVOLUTIONS {
            wheel.turn(b"xyz", 2, TieBreak::BirthOrder);
        }

        let mut log = DeathLog::new();
        log.mark(&wheel);

        // "ab" не входил в свёртку последних K оборотов и сам не
        // достижим ни от какого текущего корня -> не в фокусе.
        assert!(!log.is_focused(ab), "'ab' обязан выйти из фокуса — не был в своде последних {ROOT_WINDOW_REVOLUTIONS} оборотов и не достижим");
        // Но его дети (байты 'a','b') остаются в фокусе как часть
        // фабричного дна, независимо от судьбы 'ab'.
        assert!(log.is_focused(a));
        assert!(log.is_focused(b));
    }

    #[test]
    fn resurrection_is_a_later_focus_event_not_a_special_mechanism() {
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab", 2, TieBreak::BirthOrder);
        let mut log = DeathLog::new();
        log.mark(&wheel);

        let ab = wheel.sequence.ids.iter().copied().find(|&id| wheel.store.bytes_of(id) == b"ab").expect("'ab' должен был слиться в композит");
        assert!(log.is_focused(ab));

        for _ in 0..ROOT_WINDOW_REVOLUTIONS {
            wheel.turn(b"zzz", 2, TieBreak::BirthOrder);
        }
        log.mark(&wheel);
        assert!(!log.is_focused(ab), "должен выйти из фокуса без новых встреч");

        // Воскрешение: новая встреча возвращает токен в свёртку.
        wheel.turn(b"ab ab", 2, TieBreak::BirthOrder);
        log.mark(&wheel);
        assert!(log.is_focused(ab), "новая встреча обязана вернуть в фокус — Focus, не отдельный механизм");

        // В логе есть оба перехода на этот токен, в этом порядке.
        let transitions: Vec<FocusEvent> = log.events().iter().copied().filter(|e| matches!(e, FocusEvent::Focus { token, .. } | FocusEvent::Unfocus { token, .. } if *token == ab)).collect();
        assert!(transitions.len() >= 3, "минимум Focus->Unfocus->Focus на этот токен, получили {:?}", transitions);
    }

    #[test]
    fn mark_outcome_reports_three_natural_results() {
        let mut wheel = WheelState::new();
        wheel.turn(b"hello world", 2, TieBreak::BirthOrder);
        let mut log = DeathLog::new();
        let first = log.mark(&wheel);
        assert_eq!(first.unchanged, 0, "первый mark -> всё либо новое, либо нет");
        assert!(first.newly_focused > 0);

        let second = log.mark(&wheel); // без нового оборота -> состояние то же
        assert_eq!(second.newly_focused, 0);
        assert_eq!(second.newly_unfocused, 0);
        assert!(second.unchanged > 0, "повторный mark без изменений Колеса обязан дать unchanged, не Focus заново");
    }

    #[test]
    fn weight_cut_removes_roughly_a_quarter_of_borderline_tokens() {
        let mut wheel = WheelState::new();
        let text = b"the cat sat on the mat, the cat sat on the mat, the cat ran to the mat, \
                     the dog sat on the log, the dog sat on the log, the dog ran to the log, \
                     a bird flew over the tree, a bird flew over the tree, a bird sang in the tree";
        wheel.turn(text, 2, TieBreak::BirthOrder);
        let mut log = DeathLog::new();
        log.mark(&wheel);

        let outcome = log.weight_cut(&wheel);
        assert!(outcome.borderline_count > 0, "должны быть пограничные (достижимые не-корни) на таком тексте");
        let expected = ((outcome.borderline_count as f64) * WEIGHT_CUT_QUANTILE).round() as usize;
        assert_eq!(outcome.cut_count, expected, "срез обязан снять ровно квантиль 25% (с округлением)");
    }

    #[test]
    fn weight_cut_only_acts_on_borderline_not_on_roots() {
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab cd cd ab cd ef ef gh gh ef gh", 2, TieBreak::BirthOrder);
        let mut log = DeathLog::new();
        log.mark(&wheel);
        let roots_before = root_set(&wheel);

        log.weight_cut(&wheel);

        // Ни один корень не должен был потерять фокус от среза по весу —
        // срез оперирует только над "пограничными" (reachable \ roots).
        for &root in &roots_before {
            assert!(log.is_focused(root), "срез по весу не имеет права снять корень");
        }
    }

    #[test]
    fn replay_reconstructs_focused_set_bit_for_bit() {
        let mut wheel = WheelState::new();
        wheel.turn(b"the cat sat on the mat", 2, TieBreak::BirthOrder);
        let mut log = DeathLog::new();
        log.mark(&wheel);
        for _ in 0..ROOT_WINDOW_REVOLUTIONS + 1 {
            wheel.turn(b"zzz zzz zzz", 2, TieBreak::BirthOrder);
        }
        log.mark(&wheel);

        let replayed = DeathLog::replay(log.events());
        assert_eq!(replayed, *log.focused_set(), "replay обязан дать то же множество, что накопленное состояние");
    }

    fn tmp_db_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("shelf_death_test_{}_{}.redb", name, std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn persist_round_trip_matches_in_memory_state() {
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab cd cd ab cd", 2, TieBreak::BirthOrder);
        let mut log = DeathLog::new();
        log.mark(&wheel);
        for _ in 0..ROOT_WINDOW_REVOLUTIONS + 1 {
            wheel.turn(b"zzz", 2, TieBreak::BirthOrder);
        }
        log.mark(&wheel);

        let path = tmp_db_path("round_trip");
        save(&log, &path).expect("save");
        let loaded = load(&path).expect("load");

        assert_eq!(loaded.events(), log.events());
        assert_eq!(*loaded.focused_set(), *log.focused_set());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn incremental_save_matches_one_shot() {
        let mut wheel = WheelState::new();
        wheel.turn(b"ab ab", 2, TieBreak::BirthOrder);
        let mut log = DeathLog::new();
        log.mark(&wheel);

        let incremental_path = tmp_db_path("incremental");
        save(&log, &incremental_path).expect("первая дозапись");

        for _ in 0..ROOT_WINDOW_REVOLUTIONS + 1 {
            wheel.turn(b"cd cd", 2, TieBreak::BirthOrder);
        }
        log.mark(&wheel);
        save(&log, &incremental_path).expect("вторая дозапись");

        let one_shot_path = tmp_db_path("one_shot");
        save(&log, &one_shot_path).expect("контрольный одноразовый save");

        let loaded_incremental = load(&incremental_path).expect("load инкрементального");
        let loaded_one_shot = load(&one_shot_path).expect("load одноразового");
        assert_eq!(loaded_incremental.events(), loaded_one_shot.events());

        let _ = std::fs::remove_file(&incremental_path);
        let _ = std::fs::remove_file(&one_shot_path);
    }

    #[test]
    fn test_focus_set_snapshot() {
        let mut wheel = WheelState::new();
        wheel.turn(b"the memory engine of axmg is fast and reliable", 2, TieBreak::BirthOrder);
        let mut log = DeathLog::new();
        log.mark(&wheel);

        let snapshot = log.snapshot(&wheel, 10);
        assert_eq!(snapshot.revolution(), wheel.revolution);
        assert_eq!(snapshot.len(), log.focused_set().len());
        assert!(!snapshot.is_empty());
        for &id in log.focused_set() {
            assert!(snapshot.contains(id));
        }
    }
}
