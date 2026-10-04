use axmg::context::Window;
use axmg::merge::TieBreak;
use axmg::ppmi::{self, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD};
use axmg::second_shelf::accumulate_windowed_per_generation;
use axmg::walks::WalkGraph;
use axmg::{read_and_touch, Sequence, Store, BIRTH_THRESHOLD};
use std::path::Path;

fn report(label: &str, store: &Store, valid_links: &[axmg::ppmi::PpmiEntry]) {
    let graph = WalkGraph::build(store, valid_links, 2);
    let nodes = graph.nodes();
    let with_cooc = nodes.iter().filter(|&&n| graph.cooccurrence_degree(n) > 0).count();
    let with_struct = nodes.iter().filter(|&&n| graph.structural_degree(n) > 0).count();
    println!(
        "{label}: узлов={}, PPMI-рёбер={}, узлов с >=1 PPMI-ребром={} ({:.1}%), узлов со структурными={} ({:.1}%)",
        nodes.len(),
        valid_links.len(),
        with_cooc,
        100.0 * with_cooc as f64 / nodes.len() as f64,
        with_struct,
        100.0 * with_struct as f64 / nodes.len() as f64,
    );
}

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    for corpus_name in ["Стихи 2025.md", "bhagavad_gita.txt"] {
        let corpus_path = Path::new(manifest_dir).join(format!("../docs/corpus/{corpus_name}"));
        let bytes = std::fs::read(&corpus_path).expect("corpus");

        let mut store = Store::new();
        store.init_factory();
        let ids = read_and_touch(&mut store, &bytes);
        let seq = Sequence::from_ids(&store, ids);
        let window = Window::range(2, 5);
        let (_out, coo) = accumulate_windowed_per_generation(&mut store, seq, BIRTH_THRESHOLD, TieBreak::BirthOrder, &window, 2, 0xA4_5EED);

        println!("\n=== {corpus_name} ===");
        let csr_real = coo.real.bake_to_csr();
        let valid_real = ppmi::valid_links(&csr_real, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD);
        report("реальный", &store, &valid_real);

        let csr_permuted = coo.permuted.bake_to_csr();
        let valid_permuted = ppmi::valid_links(&csr_permuted, DEFAULT_FREQUENCY_FLOOR, DEFAULT_PPMI_THRESHOLD);
        report("переставленный", &store, &valid_permuted);
    }
}
