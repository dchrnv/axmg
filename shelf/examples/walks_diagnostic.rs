use axmg::context::Window;
use axmg::merge::TieBreak;
use axmg::ppmi::{self, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD};
use axmg::second_shelf::accumulate_windowed_per_generation;
use axmg::walks::WalkGraph;
use axmg::{read_and_touch, Sequence, Store, BIRTH_THRESHOLD};
use std::path::Path;

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let corpus_path = Path::new(manifest_dir).join("../docs/corpus/Стихи 2025.md");
    let bytes = std::fs::read(&corpus_path).expect("Стихи 2025.md");

    let mut store = Store::new();
    store.init_factory();
    let ids = read_and_touch(&mut store, &bytes);
    let seq = Sequence::from_ids(&store, ids);
    let window = Window::range(2, 5);
    let (_out, coo) = accumulate_windowed_per_generation(&mut store, seq, BIRTH_THRESHOLD, TieBreak::BirthOrder, &window, 2, 0xA4_5EED);

    let csr_real = coo.real.bake_to_csr();
    let csr_permuted = coo.permuted.bake_to_csr();
    let valid_real = ppmi::valid_links(&csr_real, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD);
    let valid_permuted = ppmi::valid_links(&csr_permuted, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD);
    println!("валидных PPMI-связей: реальный={}, переставленный={}", valid_real.len(), valid_permuted.len());

    let graph_real = WalkGraph::build(&store, &valid_real, 2);
    let graph_permuted = WalkGraph::build(&store, &valid_permuted, 2);
    println!("узлов графа: реальный={}, переставленный={}", graph_real.nodes().len(), graph_permuted.nodes().len());

    // Сколько узлов имеют структурные рёбра (общая часть для обоих графов)?
    let mut only_structural_real = 0;
    let mut only_cooc_real = 0;
    let mut both_real = 0;
    for &n in &graph_real.nodes() {
        let has_struct = graph_real.structural_degree(n) > 0;
        let has_cooc = graph_real.cooccurrence_degree(n) > 0;
        match (has_struct, has_cooc) {
            (true, false) => only_structural_real += 1,
            (false, true) => only_cooc_real += 1,
            (true, true) => both_real += 1,
            _ => {}
        }
    }
    println!("реальный: только структурные={only_structural_real}, только co-occ={only_cooc_real}, оба={both_real}");

    let mut only_structural_p = 0;
    let mut only_cooc_p = 0;
    let mut both_p = 0;
    for &n in &graph_permuted.nodes() {
        let has_struct = graph_permuted.structural_degree(n) > 0;
        let has_cooc = graph_permuted.cooccurrence_degree(n) > 0;
        match (has_struct, has_cooc) {
            (true, false) => only_structural_p += 1,
            (false, true) => only_cooc_p += 1,
            (true, true) => both_p += 1,
            _ => {}
        }
    }
    println!("переставленный: только структурные={only_structural_p}, только co-occ={only_cooc_p}, оба={both_p}");
}
