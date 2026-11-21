use std::{error::Error, hint::black_box, path::Path, sync::atomic::{AtomicU64,Ordering}, time::Instant};
mod input;
mod block;
type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn main() -> Result<()> {
    let files = input::files(Path::new("/tmp/shop-input/events"))?;
    let args: Vec<_> = std::env::args().collect();
    for mode in ["fields","dates","dates","fields","fields","dates"] {
        let total = AtomicU64::new(0);
        let start = Instant::now();
        input::scan_chunks(&files, |chunk| {
            let mut dates = input::Dates::default();
            let mut checksum = 0_u64;
            let mut row = |values: [&[u8]; 7]| {
                let values = black_box(values);
                let n = if mode == "fields" { values[0].len() as u64 } else {
                    (dates.parse(values[3]).unwrap_or(0) ^ dates.parse(values[2]).unwrap_or(0)) as u64
                };
                checksum = checksum.wrapping_add(n);
            };
            if args[1]=="scalar" { input::data_rows(chunk).for_each(row); } else {
                input::visit_rows(chunk, &mut row);
            }
            total.fetch_add(checksum,Ordering::Relaxed);
        })?;
        println!("DECODE {} {mode} {:.4} {}",args[1],start.elapsed().as_secs_f64(),total.into_inner());
    }
    Ok(())
}
