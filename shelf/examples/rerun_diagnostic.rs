use shelf::merge::{self, Sequence, TieBreak};
use shelf::{read_and_touch, Store, BIRTH_THRESHOLD};
use std::path::Path;

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let corpus_path = Path::new(manifest_dir).join("../docs/corpus/bhagavad_gita.txt");
    let bytes = std::fs::read(&corpus_path).expect("bhagavad_gita.txt");

    let mut store = Store::new();
    store.init_factory();
    let ids = read_and_touch(&mut store, &bytes);
    let seq = Sequence::from_ids(&store, ids);
    let out1 = merge::run(&mut store, seq, BIRTH_THRESHOLD, TieBreak::BirthOrder);
    println!("после run() #1: токенов в сторе={}, длина последовательности={}", store.len(), out1.ids.len());

    // Скормить результат ЕЩЁ РАЗ через run() на том же сторе — если
    // цикл поколений остановился преждевременно (births==0 при
    // оставшихся мержабельных парах), это найдёт и досклеит их.
    let seq2 = Sequence { ids: out1.ids.clone(), levels: out1.levels.clone() };
    let out2 = merge::run(&mut store, seq2, BIRTH_THRESHOLD, TieBreak::BirthOrder);
    println!("после run() #2 (повторно на результате #1): токенов в сторе={}, длина последовательности={}", store.len(), out2.ids.len());

    if out2.ids.len() < out1.ids.len() {
        println!("!!! ПОДТВЕРЖДЕНО: повторный прогон нашёл ещё слияния -> цикл поколений останавливается преждевременно");
    } else {
        println!("повторный прогон ничего не изменил -> гипотеза неверна, каскад действительно сошёлся");
    }
}
