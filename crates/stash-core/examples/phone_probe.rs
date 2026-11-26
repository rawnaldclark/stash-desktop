// Lists the phones Windows sees. With a phone's name and a folder, it copies the files of that phone's transfer folder
// (not the list) there, as the core takes them (each read watched for stalls), and says how fast each came, then
// deletes the copy. Only the phone named is opened; nothing goes into a library. With `--delete`, each file is also
// deleted from the phone once copied, as a transfer does: for test files only.
//
//     cargo run -p stash-core --example phone_probe -- "Pixel 5" C:\scratch\probe 20 [--delete]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let phones = stash_core::phone::phones();
    println!(
        "phones: {:?}",
        phones
            .as_ref()
            .map(|p| p.iter().map(|p| p.name.clone()).collect::<Vec<_>>())
    );
    let (Some(name), Some(dir)) = (args.first(), args.get(1)) else {
        return;
    };
    #[cfg(windows)]
    for p in phones.unwrap_or_default().into_iter().filter(|p| &p.name == name) {
        let t = match stash_core::phone::open_transfer(&p.id) {
            Ok(Some(t)) => t,
            Ok(None) => {
                println!("{}: no transfer folder (or no storage shown)", p.name);
                continue;
            }
            Err(e) => {
                println!("{}: {e}", p.name);
                continue;
            }
        };
        let total: u64 = t.files.iter().map(|f| f.size).sum();
        println!(
            "{}: transfer folder with {} files, {} MB; list there: {}",
            p.name,
            t.files.len(),
            total / 1_000_000,
            t.files.iter().any(|f| f.name == stash_core::phone::LIST)
        );
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("the folder to copy into");
        let n: usize = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(10);
        let delete = args.iter().any(|a| a == "--delete");
        let (mut bytes, mut failed, started) = (0u64, 0usize, std::time::Instant::now());
        for (i, f) in t.files.iter().filter(|f| f.name != stash_core::phone::LIST).take(n).enumerate() {
            let at = std::time::Instant::now();
            let stalled = || println!("  {}: sent nothing for a while: calling the read off", f.name);
            let watch = stash_core::phone::Watch {
                stall_after: stash_core::phone::STALL,
                progress: &|_| {},
                stalled: &stalled,
            };
            let to = dir.join(&f.name);
            match t.copy(f, &to, &watch) {
                Ok(got) => {
                    bytes += got;
                    let secs = at.elapsed().as_secs_f64();
                    if i % 25 == 0 || secs > 10.0 {
                        println!(
                            "  #{i} {} {} MB in {secs:.1} s ({:.1} MB/s); {:.0} s in",
                            f.name,
                            got / 1_000_000,
                            got as f64 / 1e6 / secs.max(0.001),
                            started.elapsed().as_secs_f64()
                        );
                    }
                    if delete && let Err(e) = t.remove_file(f) {
                        println!("  #{i} {}: not deleted from the phone: {e}", f.name);
                    }
                }
                Err(e) => {
                    failed += 1;
                    println!("  #{i} {}: {e} after {:.1} s", f.name, at.elapsed().as_secs_f64());
                    if failed >= 3 {
                        println!("  three failures: stopping");
                        break;
                    }
                }
            }
            let _ = std::fs::remove_file(&to);
        }
        let secs = started.elapsed().as_secs_f64();
        println!(
            "  all: {} MB in {secs:.1} s ({:.1} MB/s), {failed} failed",
            bytes / 1_000_000,
            bytes as f64 / 1e6 / secs.max(0.001)
        );
    }
}
