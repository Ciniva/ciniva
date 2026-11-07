fn main() {
    for p in std::env::args().skip(1) {
        let k = ciniva::index::keyframes(std::path::Path::new(&p));
        println!("{}", serde_json::to_string(&k).unwrap());
    }
}
