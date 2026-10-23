//! Read-only catalog paging probe for an existing local v3 store.

use std::error::Error;
use std::path::Path;
use std::time::{Duration, Instant};

use seex::{ProjectId, Reader};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let root = args.next().ok_or("expected store path")?;
    let project = ProjectId::from_string(args.next().unwrap_or_else(|| "p2".to_owned()));
    let reader = Reader::builder(Path::new(&root)).open()?;
    let page = reader.runs_page(&project, None, 100)?;
    if page.len() != 100 {
        return Err(format!("expected 100 Runs, found {}", page.len()).into());
    }
    let started = Instant::now();
    let mut samples = Vec::with_capacity(10);
    for _ in 0..10 {
        let sample_started = Instant::now();
        let mut operations = 0;
        while sample_started.elapsed() < Duration::from_millis(25) {
            let next = reader.runs_page(&project, page.last(), 100)?;
            if next.len() != 100 {
                return Err(
                    format!("expected 100 Runs on the next page, found {}", next.len()).into(),
                );
            }
            operations += 1;
        }
        samples.push(sample_started.elapsed().as_nanos() / operations);
        if started.elapsed() >= Duration::from_secs(110) {
            return Err("paging probe exceeded the bounded measurement window".into());
        }
    }
    println!("{{\"page_size\":100,\"samples_ns\":{samples:?}}}");
    Ok(())
}
