//! Шаг 11 (A3, план 3): PPMI — фильтр значимости связи.
//!
//! `PPMI(a,b) = max(0, log2( p(a,b) / (p(a)·p(b)) ))`. Классическая
//! формула на маргиналах CSR (шаг 9): `p(a,b) = w(a,b)/total`,
//! `p(a) = row_marginal(a)/total`, `p(b) = col_marginal(b)/total` — что
//! после сокращения даёт `log2( w(a,b)·total / (row_marginal(a)·
//! col_marginal(b)) )`. PMI ≈ 0 — «часто вместе, потому что оба часты
//! по отдельности» — мусор; фильтруется порогом, не просто взятием max.
//!
//! **Частотный пол:** пары с сырым счётом < `frequency_floor` НЕ
//! оцениваются вовсе (план 3, шаг 11) — возвращается `None`, не
//! `PPMI = 0`. Та же логика, что анти-CMS-оговорка шага 9: PMI на редких
//! парах ненадёжен по построению (маленький числитель — шумная оценка),
//! оценивать его — заводить дутый сигнал ровно там, где фильтр важнее
//! всего.
//!
//! **Контроль — перемешивание** (пермутационный нуль-эксперимент):
//! тот же корпус с перетасованными токенами; на перемешанном
//! распределение PPMI обязано схлопнуться. (Исправлено: здесь раньше
//! стояла ссылка на корпус "Чехов" как аналог контроля этапа 1 — такого
//! корпуса нет и не было ни на этапе 1 (там held-out был Стихи↔Гита),
//! ни где-либо в репозитории; ссылка была придумана и закреплена
//! самоцитированием в план 3.md, см. Правку 5 там же.) Сам шаффл —
//! ответственность вызывающего кода эксперимента A4 (там, где строится
//! COO из перемешанного потока), не этого модуля — `ppmi.rs` считает
//! число по уже готовому CSR и порогам, ничего не знает про то, откуда
//! CSR взялся.

use crate::cooc::Csr;
use crate::store::TokenId;

/// Пара с сырым счётом ниже этого порога не оценивается (план 3, шаг 11).
pub const DEFAULT_FREQUENCY_FLOOR: u64 = 3;
/// Порог валидности связи (план 3, шаг 11: "предложение — PPMI ≥ 2.0 бита").
pub const DEFAULT_PPMI_THRESHOLD: f64 = 2.0;

/// PPMI одной пары. `None`, если сырой счёт ниже частотного пола, либо
/// один из маргиналов равен нулю (не может встретиться, если `weight >
/// 0` и CSR баланс верен — но проверяется явно, не через панику при
/// делении на 0).
pub fn ppmi(csr: &Csr, token: TokenId, context: TokenId, frequency_floor: u64) -> Option<f64> {
    let w = csr.weight_of(token, context);
    if w < frequency_floor {
        return None;
    }
    let row = *csr.row_marginal.get(&token)?;
    let col = *csr.col_marginal.get(&context)?;
    if row == 0 || col == 0 || csr.total_weight == 0 {
        return None;
    }
    let pmi = ((w as f64) * (csr.total_weight as f64) / ((row as f64) * (col as f64))).log2();
    Some(pmi.max(0.0))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PpmiEntry {
    pub token: TokenId,
    pub context: TokenId,
    pub raw_weight: u64,
    pub ppmi: f64,
}

/// Пройти весь CSR в детерминированном порядке (строки и колонки внутри
/// строки уже отсортированы при запекании — Р2), оценить PPMI на парах,
/// прошедших частотный пол, вернуть только валидные (PPMI >= threshold).
pub fn valid_links(csr: &Csr, frequency_floor: u64, threshold: f64) -> Vec<PpmiEntry> {
    let mut out = Vec::new();
    for &token in &csr.row_index {
        for (context, weight) in csr.row(token) {
            if let Some(p) = ppmi(csr, token, context, frequency_floor) {
                if p >= threshold {
                    out.push(PpmiEntry {
                        token,
                        context,
                        raw_weight: weight,
                        ppmi: p,
                    });
                }
            }
        }
    }
    out
}

/// (валидных, оценённых-всего) среди пар, прошедших частотный пол.
/// Пары, не прошедшие пол, не входят ни в числитель, ни в знаменатель —
/// это метрика 1 эксперимента A4 (план 3): "доля связей над порогом PPMI
/// среди пар, прошедших частотный пол".
pub fn valid_fraction(csr: &Csr, frequency_floor: u64, threshold: f64) -> (usize, usize) {
    let mut evaluated = 0usize;
    let mut valid = 0usize;
    for &token in &csr.row_index {
        for (context, _weight) in csr.row(token) {
            if let Some(p) = ppmi(csr, token, context, frequency_floor) {
                evaluated += 1;
                if p >= threshold {
                    valid += 1;
                }
            }
        }
    }
    (valid, evaluated)
}

/// Гистограмма PPMI по фиксированным бинам ширины `bin_width`, начиная с
/// 0 (PPMI неотрицателен по построению). Последний бин собирает "хвост"
/// (>= `bin_width * (bins.len()-1)`). Для снапшот-отчёта (план 3, шаг 11).
pub fn histogram(csr: &Csr, frequency_floor: u64, bin_width: f64, bins: usize) -> Vec<usize> {
    assert!(bin_width > 0.0 && bins > 0);
    let mut hist = vec![0usize; bins];
    for &token in &csr.row_index {
        for (context, _weight) in csr.row(token) {
            if let Some(p) = ppmi(csr, token, context, frequency_floor) {
                let bin = ((p / bin_width) as usize).min(bins - 1);
                hist[bin] += 1;
            }
        }
    }
    hist
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cooc::CooAccumulator;

    /// Синтетический COO с двумя независимыми "кластерами корреляции":
    /// (A,B) и (Y,X) часто встречаются вместе (сильнее, чем предсказали бы
    /// их отдельные частоты), (A,X) и (Y,B) — реже, чем предсказали бы
    /// (анти-корреляция, PPMI обязан схлопнуться в 0 через max(0,·)).
    fn correlated_csr() -> Csr {
        let mut acc = CooAccumulator::new();
        acc.accumulate(&[(1, 2, 40), (1, 8, 5), (9, 2, 5), (9, 8, 40)]); // A=1,B=2,X=8,Y=9
        acc.bake_to_csr()
    }

    #[test]
    fn correlated_pair_has_positive_ppmi() {
        let csr = correlated_csr();
        let p = ppmi(&csr, 1, 2, 3).expect("вес 40 >= пола 3");
        // Ручной расчёт: total=90, row(1)=45, col(2)=45, w=40.
        // pmi = log2(40*90/(45*45)) = log2(3600/2025) = log2(1.7778) ≈ 0.83
        assert!((p - 0.830_074_998_557_687_5).abs() < 1e-9, "got {p}");
        assert!(p > 0.0);
    }

    #[test]
    fn anticorrelated_pair_floors_to_zero_not_negative() {
        let csr = correlated_csr();
        let p = ppmi(&csr, 1, 8, 3).expect("вес 5 >= пола 3");
        assert_eq!(p, 0.0, "анти-корреляция должна схлопнуться в 0 через max(0,·), не остаться отрицательной");
    }

    #[test]
    fn below_frequency_floor_is_not_evaluated_at_all() {
        let mut acc = CooAccumulator::new();
        acc.accumulate(&[(1, 2, 2)]); // вес 2 < пола по умолчанию (3)
        let csr = acc.bake_to_csr();
        assert_eq!(
            ppmi(&csr, 1, 2, DEFAULT_FREQUENCY_FLOOR),
            None,
            "пара ниже пола обязана вернуть None, не PPMI=0 — это разные утверждения"
        );
    }

    #[test]
    fn valid_links_respects_threshold_and_floor() {
        let csr = correlated_csr();
        let valid = valid_links(&csr, 3, 0.5);
        // (1,2) и (9,8) должны пройти (PPMI≈0.83 >= 0.5); (1,8) и (9,2) — нет (PPMI=0).
        assert_eq!(valid.len(), 2);
        assert!(valid.iter().any(|e| e.token == 1 && e.context == 2));
        assert!(valid.iter().any(|e| e.token == 9 && e.context == 8));
    }

    #[test]
    fn valid_fraction_counts_only_evaluated_pairs() {
        let csr = correlated_csr();
        // 4 пары, все проходят частотный пол (веса 40,5,5,40 >= 3).
        let (valid, evaluated) = valid_fraction(&csr, 3, 2.0);
        assert_eq!(evaluated, 4);
        assert_eq!(valid, 0, "порог 2.0 бита выше достигнутого PPMI≈0.83 — ни одна пара не проходит");

        let (valid2, evaluated2) = valid_fraction(&csr, 3, 0.5);
        assert_eq!(evaluated2, 4);
        assert_eq!(valid2, 2);
    }

    #[test]
    fn histogram_bins_sum_to_evaluated_count() {
        let csr = correlated_csr();
        let hist = histogram(&csr, 3, 0.25, 10);
        let (_, evaluated) = valid_fraction(&csr, 3, 0.0);
        assert_eq!(hist.iter().sum::<usize>(), evaluated);
    }
}
