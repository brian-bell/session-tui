use session_tui::sessions::{scan_all_sessions, ScanRoots};

fn main() {
    let start = std::time::Instant::now();
    let sessions = scan_all_sessions(&ScanRoots::default()).unwrap();
    let elapsed = start.elapsed();
    for s in sessions.iter().take(8) {
        println!(
            "{:?} {} {} [{}] {}",
            s.agent,
            &s.id[..8],
            s.timestamp.format("%m-%d %H:%M"),
            s.cwd,
            s.title.chars().take(60).collect::<String>()
        );
    }
    println!("--- {} sessions in {elapsed:?}", sessions.len());
}
