use ciniva::{library::{resolve, Resolution}, parser, tmdb::Tmdb};
use futures::StreamExt;
use std::{io::BufRead, path::PathBuf};

#[tokio::main]
async fn main() {
    let tmdb = Tmdb::new(std::env::var("TMDB_API_KEY").expect("TMDB_API_KEY"), "en-US".into());
    let paths: Vec<PathBuf> = std::io::stdin().lock().lines().map_while(Result::ok).map(PathBuf::from).collect();
    let guesses = parser::parse_batch(&paths);
    let jobs: Vec<_> = paths.iter().zip(guesses).filter(|(p, _)| !parser::is_extra(p)).collect();
    let results: Vec<_> = futures::stream::iter(jobs)
        .map(|(path, guess)| { let tmdb = tmdb.clone(); async move {
            let r = resolve(&tmdb, &guess, None, &path).await;
            (path.clone(), r)
        }})
        .buffered(6)
        .collect()
        .await;
    for (path, r) in results {
        let out = match r {
            Ok(Resolution::Movie(id)) => {
                let m = tmdb.movie(id).await.unwrap();
                format!("MOVIE\t{} ({})", m.title, m.release_date.unwrap_or_default().get(..4).unwrap_or(""))
            }
            Ok(Resolution::Episodes { tv, episodes }) => {
                let t = tmdb.tv(tv).await.unwrap();
                format!("TV\t{} {:?}", t.name, episodes)
            }
            Ok(Resolution::Unmatched(n)) => format!("UNMATCHED\t{n}"),
            Err(e) => format!("ERROR\t{e:#}"),
        };
        println!("{out}\t{}", path.display());
    }
}
