//! End-to-end checks of the status bar: Optimus runs inside a pseudoconsole of its own,
//! and the test reads the screen it draws, types into it, and presses Ctrl+C.

use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use avt::Vt;
use optimus::pty::{command_line, ConPty};

const COLS: u16 = 100;
const ROWS: u16 = 30;

struct Harness {
    pty: Arc<ConPty>,
    rx: mpsc::Receiver<Vec<u8>>,
    vt: Vt,
    pending: Vec<u8>,
    _home: tempfile::TempDir,
}

impl Harness {
    fn start(args: &[&str]) -> Self {
        let home = tempfile::tempdir().unwrap();
        // Child processes inherit this; each test binary runs its tests in one process,
        // and every test points it at its own directory before spawning.
        std::env::set_var("OPTIMUS_HOME", home.path());
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let line = command_line(Path::new(env!("CARGO_BIN_EXE_optimus")), &args);
        let pty = Arc::new(ConPty::spawn_suspended(&line, COLS, ROWS).expect("spawn"));
        pty.resume().expect("resume");
        let (tx, rx) = mpsc::channel();
        let reader = pty.clone();
        thread::spawn(move || {
            let mut buf = vec![0u8; 65536];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            pty,
            rx,
            vt: Vt::new(COLS as usize, ROWS as usize),
            pending: Vec::new(),
            _home: home,
        }
    }

    fn pump(&mut self, for_how_long: Duration) {
        let until = Instant::now() + for_how_long;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match self.rx.recv_timeout(left.min(Duration::from_millis(50))) {
                Ok(bytes) => {
                    self.pending.extend_from_slice(&bytes);
                    let text = optimus::session::decode_utf8(&mut self.pending);
                    self.vt.feed_str(&text);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
    }

    fn screen(&self) -> String {
        self.vt.text().join("\n")
    }

    /// Pump until `needle` is on screen or the timeout passes.
    fn wait_for(&mut self, needle: &str, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        while Instant::now() < until {
            self.pump(Duration::from_millis(100));
            if self.screen().contains(needle) {
                return true;
            }
        }
        false
    }

    fn type_text(&self, text: &str) {
        self.pty.write(text.as_bytes()).expect("write input");
    }

    fn wait_exit(&mut self, timeout: Duration) -> Option<i32> {
        let until = Instant::now() + timeout;
        while Instant::now() < until {
            self.pump(Duration::from_millis(100));
            if self.pty.has_exited() {
                self.pump(Duration::from_millis(300));
                return self.pty.exit_code();
            }
        }
        None
    }
}

#[test]
fn bar_under_agent_output_and_summary_after() {
    let mut h = Harness::start(&[
        "run",
        "--budget",
        "2G",
        "--",
        "cmd",
        "/c",
        "echo hello-from-agent & ping -n 4 127.0.0.1 >nul",
    ]);
    assert!(
        h.wait_for("2.0 GB soft", Duration::from_secs(10)),
        "bar never appeared:\n{}",
        h.screen()
    );
    let screen = h.screen();
    assert!(screen.contains("hello-from-agent"), "{screen}");
    let last = screen.lines().last().unwrap_or_default();
    assert!(
        last.starts_with(" optimus "),
        "bar not on the last row: {last:?}"
    );
    assert!(last.contains("PC ") && last.contains("agents"), "{last:?}");

    assert_eq!(h.wait_exit(Duration::from_secs(20)), Some(0));
    let screen = h.screen();
    assert!(
        screen.contains("optimus: cmd exited with code 0"),
        "{screen}"
    );
    assert!(
        !screen.contains("2.0 GB soft"),
        "bar left behind:\n{screen}"
    );
}

#[test]
fn keystrokes_reach_the_agent() {
    let mut h = Harness::start(&["run", "--", "cmd", "/q", "/k", "prompt $g"]);
    assert!(
        h.wait_for(" optimus ", Duration::from_secs(10)),
        "{}",
        h.screen()
    );
    h.type_text("echo typed-through-optimus\r");
    assert!(
        h.wait_for("\ntyped-through-optimus", Duration::from_secs(10)),
        "{}",
        h.screen()
    );
    h.type_text("exit 3\r");
    assert_eq!(h.wait_exit(Duration::from_secs(10)), Some(3));
}

#[test]
fn ctrl_c_goes_to_the_agent_not_optimus() {
    let mut h = Harness::start(&[
        "run",
        "--",
        "powershell",
        "-NoProfile",
        "-Command",
        // A stopping pipeline drops normal output, so write straight to the console.
        "'agent-ready'; try { Start-Sleep 30 } finally { [Console]::WriteLine('agent-cleanup-ran') }",
    ]);
    assert!(
        h.wait_for("agent-ready", Duration::from_secs(15)),
        "{}",
        h.screen()
    );
    h.type_text("\x03");
    let code = h.wait_exit(Duration::from_secs(15));
    let screen = h.screen();
    assert!(screen.contains("agent-cleanup-ran"), "{screen}");
    // Optimus survived Ctrl+C long enough to report the agent's exit.
    assert!(screen.contains("optimus: powershell exited"), "{screen}");
    assert!(code.is_some());
}

/// Needs Claude Code installed; makes no model call. `cargo test --test bar -- --ignored --nocapture`
#[test]
#[ignore]
fn real_claude_code_renders_with_bar() {
    let mut h = Harness::start(&["run", "--budget", "3G", "--", "claude"]);
    let shown = h.wait_for("3.0 GB soft", Duration::from_secs(20));
    h.pump(Duration::from_secs(6));
    println!("----- screen -----\n{}\n------------------", h.screen());
    assert!(shown, "bar never appeared");
    let last = h.screen().lines().last().unwrap_or_default().to_string();
    assert!(last.starts_with(" optimus "), "{last:?}");
    // Two Ctrl+C presses quit Claude Code.
    h.type_text("\x03");
    h.pump(Duration::from_millis(300));
    h.type_text("\x03");
    let code = h.wait_exit(Duration::from_secs(15));
    println!("----- after exit -----\n{}", h.screen());
    assert!(code.is_some(), "claude did not exit on Ctrl+C Ctrl+C");
}

/// Needs Codex installed; makes no model call. `cargo test --test bar -- --ignored --nocapture`
#[test]
#[ignore]
fn real_codex_renders_with_bar() {
    let mut h = Harness::start(&["run", "--budget", "2G", "--", "codex"]);
    let shown = h.wait_for("2.0 GB soft", Duration::from_secs(20));
    h.pump(Duration::from_secs(6));
    println!("----- screen -----\n{}\n------------------", h.screen());
    assert!(shown, "bar never appeared");
    h.type_text("\x03");
    h.pump(Duration::from_millis(300));
    h.type_text("\x03");
    let code = h.wait_exit(Duration::from_secs(15));
    println!("----- after exit -----\n{}", h.screen());
    assert!(code.is_some(), "codex did not exit on Ctrl+C Ctrl+C");
}

#[test]
fn long_output_scrolls_into_history() {
    let mut h = Harness::start(&[
        "run",
        "--",
        "cmd",
        "/c",
        "for /l %i in (1,1,80) do @echo line-%i",
    ]);
    assert_eq!(h.wait_exit(Duration::from_secs(20)), Some(0));
    let screen = h.screen();
    // The last lines are on screen, in order, above the summary.
    let l79 = screen.find("line-79").expect("line-79 visible");
    let l80 = screen.find("line-80").expect("line-80 visible");
    assert!(l79 < l80, "{screen}");
}
