use shelf::context::{ContextExtractor, Neighbor, Parent, Window};
use shelf::cooc::CooAccumulator;
use shelf::merge::{self, TieBreak};
use shelf::{read_and_touch, Sequence, Store, BIRTH_THRESHOLD};
use std::path::Path;

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let corpus_path = Path::new(manifest_dir).join("../docs/corpus/bhagavad_gita.txt");
    let bytes = std::fs::read(&corpus_path).expect("bhagavad_gita.txt");

    let mut store = Store::new();
    store.init_factory();
    let ids = read_and_touch(&mut store, &bytes);
    let seq = Sequence::from_ids(&store, ids);
    let out = merge::run(&mut store, seq, BIRTH_THRESHOLD, TieBreak::BirthOrder);
    println!("токенов: {}, длина последовательности: {}", store.len(), out.ids.len());
    println!("средняя встречаемость токена (count) в финальной последовательности: {:.3}",
        out.ids.len() as f64 / store.len() as f64);

    let window5 = Window::new(5);
    let extractors: Vec<(&str, &dyn ContextExtractor)> =
        vec![("Neighbor", &Neighbor), ("Window(5)", &window5), ("Parent", &Parent)];
    for (label, ex) in extractors {
        let events = ex.extract(&store, &out.ids);
        let mut acc = CooAccumulator::new();
        acc.accumulate(&events);
        let csr = acc.bake_to_csr();
        let max_weight = csr.values.iter().copied().max().unwrap_or(0);
        let hist: std::collections::BTreeMap<u64, usize> = csr.values.iter().fold(Default::default(), |mut m, &w| { *m.entry(w).or_insert(0) += 1; m });
        println!("\n{label}: событий={}, различных пар={}, макс.вес={}", events.len(), acc.distinct_pairs(), max_weight);
        println!("  гистограмма весов (топ-10 по частоте значения): {:?}", hist.iter().take(10).collect::<Vec<_>>());
        let above_floor = csr.values.iter().filter(|&&w| w >= 3).count();
        println!("  пар с весом >= 3 (частотный пол): {} из {}", above_floor, csr.values.len());
    }
}
