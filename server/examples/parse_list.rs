use std::io::BufRead;
use std::path::Path;

fn main() {
    let flat = std::env::args().any(|a| a == "--flat");
    let lines: Vec<String> = std::io::stdin().lock().lines().map_while(Result::ok).collect();
    let targets: Vec<_> = lines.iter().map(|l| { let p = Path::new(l); if flat { Path::new("/dump").join(p.file_name().unwrap()) } else { p.to_path_buf() } }).collect();
    let guesses = ciniva::parser::parse_batch(&targets);
    for ((line, target), guess) in lines.iter().zip(&targets).zip(&guesses) {
        if ciniva::parser::is_extra(target) {
            println!("EXTRA\t{line}");
            continue;
        }
        println!("{}\t{line}", serde_json::to_string(&guess).unwrap());
    }
}
