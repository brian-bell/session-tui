use session_tui::sessions::{ScanRoots, Scanner};

fn main() {
    let roots = ScanRoots::default();
    let mut scanner = Scanner::new();

    let start = std::time::Instant::now();
    let sessions = scanner.scan(&roots).unwrap();
    let cold = start.elapsed();

    let start = std::time::Instant::now();
    let warm_sessions = scanner.scan(&roots).unwrap();
    let warm = start.elapsed();
    assert_eq!(sessions.len(), warm_sessions.len());

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
    println!("--- {} sessions: cold {cold:?}, warm rescan {warm:?}", sessions.len());
}
