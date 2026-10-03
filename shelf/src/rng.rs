//! Детерминированный PRNG (Р2, план 3): фиксированный seed → стабильный,
//! воспроизводимый вывод. SplitMix64 — минимальный, хорошо изученный
//! генератор (используется как seed-источник внутри самого `rand`).
//! Не тянем крейт `rand` ради шаффл-контроля (A4) и будущих блужданий
//! (шаг 10): `rand`'s `StdRng` явно не гарантирует стабильность вывода
//! между релизами крейта — золотой fixture (Р2) не может стоять на
//! generator'е, которому позволено тихо поменяться под ногами.

pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Равномерно в `[0, n)`. Небольшое modulo-смещение допустимо — это
    /// не криптографический контекст, а шаффл-контроль/блуждания.
    pub fn next_below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next_u64() % n
    }
}

/// Fisher-Yates на месте, детерминированный порядок при фиксированном seed.
pub fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut rng = SplitMix64::new(seed);
    let n = items.len();
    for i in (1..n).rev() {
        let j = rng.next_below((i + 1) as u64) as usize;
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_gives_same_sequence() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = SplitMix64::new(1);
        let mut b = SplitMix64::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn shuffle_preserves_multiset() {
        let mut items: Vec<u32> = (0..1000).collect();
        let original = items.clone();
        shuffle(&mut items, 12345);
        let mut sorted = items.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, original, "шаффл переставляет, не теряет и не дублирует элементы");
    }

    #[test]
    fn shuffle_is_deterministic_for_fixed_seed() {
        let mut a: Vec<u32> = (0..500).collect();
        let mut b = a.clone();
        shuffle(&mut a, 777);
        shuffle(&mut b, 777);
        assert_eq!(a, b);
    }

    #[test]
    fn shuffle_actually_moves_elements() {
        let mut items: Vec<u32> = (0..1000).collect();
        let original = items.clone();
        shuffle(&mut items, 999);
        assert_ne!(items, original, "на 1000 элементах шанс остаться на месте пренебрежимо мал");
    }
}
