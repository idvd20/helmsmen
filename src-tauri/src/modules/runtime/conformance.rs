//! Runtime conformance suite (task #6 AC).
//!
//! Every case takes `&dyn Runtime`, so any future implementation (Tmux at
//! M4) passes the exact same battery: add `#[test]`s that call these
//! functions with the new runtime. The LocalPty invocations live at the
//! bottom.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use super::{OutputSink, Runtime, SessionStatus, SpawnSpec};

const DEADLINE: Duration = Duration::from_secs(10);

fn sink() -> (OutputSink, Receiver<Vec<u8>>, Receiver<i32>) {
    let (out_tx, out_rx) = channel::<Vec<u8>>();
    let (exit_tx, exit_rx) = channel::<i32>();
    let sink = OutputSink {
        on_output: Box::new(move |bytes| {
            let _ = out_tx.send(bytes.to_vec());
        }),
        on_exit: Box::new(move |code| {
            let _ = exit_tx.send(code);
        }),
    };
    (sink, out_rx, exit_rx)
}

fn spec(program: &str, args: &[&str]) -> SpawnSpec {
    SpawnSpec {
        program: program.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        env: Default::default(),
        cols: 80,
        rows: 24,
    }
}

/// Drain output until `pred` matches the accumulated bytes or the
/// deadline passes; panics with the transcript on timeout.
fn wait_for(rx: &Receiver<Vec<u8>>, pred: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<u8> = Vec::new();
    loop {
        if pred(&seen) {
            return seen;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(chunk) => seen.extend_from_slice(&chunk),
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                if pred(&seen) {
                    return seen;
                }
                panic!(
                    "conformance timeout; transcript so far: {:?}",
                    String::from_utf8_lossy(&seen)
                );
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len().max(1)).any(|w| w == needle)
}

fn wait_exit(rx: &Receiver<i32>) -> i32 {
    rx.recv_timeout(DEADLINE).expect("session must exit in time")
}

fn wait_status(rt: &dyn Runtime, session: &str, pred: impl Fn(SessionStatus) -> bool) -> SessionStatus {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let status = rt.status(session).expect("status must stay queryable");
        if pred(status) {
            return status;
        }
        assert!(Instant::now() < deadline, "status never matched; last: {status:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// --- the suite ---

/// Spawn streams output, and the spec's env reaches the process.
pub(crate) fn case_spawn_streams_output_with_env(rt: &dyn Runtime) {
    let (sink, out, _exit) = sink();
    let mut spec = spec("/bin/sh", &["-c", r#"printf 'out:%s\n' "$HELMSMEN_CONFORMANCE""#]);
    spec.env
        .insert("HELMSMEN_CONFORMANCE".to_string(), "e2e".to_string());
    let id = rt.spawn(spec, sink).unwrap();
    wait_for(&out, |seen| contains(seen, b"out:e2e"));
    let _ = rt.kill(&id);
}

/// Writes reach the process's stdin (type into it).
pub(crate) fn case_write_reaches_stdin(rt: &dyn Runtime) {
    let (sink, out, _exit) = sink();
    let id = rt
        .spawn(
            spec("/bin/sh", &["-c", r#"read line; printf 'typed:%s\n' "$line""#]),
            sink,
        )
        .unwrap();
    rt.write(&id, b"hello\r").unwrap();
    wait_for(&out, |seen| contains(seen, b"typed:hello"));
}

/// Resize is observable from inside the PTY.
pub(crate) fn case_resize_is_observed(rt: &dyn Runtime) {
    let (sink, out, _exit) = sink();
    let id = rt.spawn(spec("/bin/sh", &[]), sink).unwrap();
    rt.resize(&id, 111, 33).unwrap();
    rt.write(&id, b"stty size\r").unwrap();
    wait_for(&out, |seen| contains(seen, b"33 111"));
    rt.kill(&id).unwrap();
}

/// Status transitions running -> exited and carries the exit code; the
/// sink's on_exit fires with the same code.
pub(crate) fn case_status_reports_exit_code(rt: &dyn Runtime) {
    let (sink, _out, exit) = sink();
    let id = rt
        .spawn(spec("/bin/sh", &["-c", "sleep 0.2; exit 7"]), sink)
        .unwrap();
    assert_eq!(rt.status(&id).unwrap(), SessionStatus::Running);
    assert_eq!(wait_exit(&exit), 7);
    wait_status(rt, &id, |s| s == SessionStatus::Exited(7));
}

/// Kill terminates a running session; kill is idempotent after exit.
pub(crate) fn case_kill_terminates(rt: &dyn Runtime) {
    let (sink, _out, exit) = sink();
    let id = rt.spawn(spec("/bin/sh", &["-c", "sleep 30"]), sink).unwrap();
    assert_eq!(rt.status(&id).unwrap(), SessionStatus::Running);
    rt.kill(&id).unwrap();
    let _ = wait_exit(&exit);
    wait_status(rt, &id, |s| matches!(s, SessionStatus::Exited(_)));
    rt.kill(&id).unwrap();
}

/// Every operation rejects an unknown session id.
pub(crate) fn case_unknown_session_ids_error(rt: &dyn Runtime) {
    let (sink, _out, _exit) = sink();
    assert!(rt.attach("ghost", sink).is_err());
    assert!(rt.write("ghost", b"x").is_err());
    assert!(rt.resize("ghost", 80, 24).is_err());
    assert!(rt.status("ghost").is_err());
    assert!(rt.kill("ghost").is_err());
}

/// Attach on a live session: the new sink gets the scrollback first, then
/// live output.
pub(crate) fn case_attach_replays_scrollback_then_streams(rt: &dyn Runtime) {
    let (sink_a, out_a, _exit_a) = sink();
    let id = rt
        .spawn(
            spec(
                "/bin/sh",
                &["-c", r#"echo first; read line; printf 'second:%s\n' "$line""#],
            ),
            sink_a,
        )
        .unwrap();
    wait_for(&out_a, |seen| contains(seen, b"first"));

    let (sink_b, out_b, _exit_b) = sink();
    rt.attach(&id, sink_b).unwrap();
    wait_for(&out_b, |seen| contains(seen, b"first"));
    rt.write(&id, b"go\r").unwrap();
    wait_for(&out_b, |seen| contains(seen, b"second:go"));
}

/// Attach after exit still hands over the scrollback and reports the exit.
pub(crate) fn case_attach_after_exit_replays_and_reports_exit(rt: &dyn Runtime) {
    let (sink_a, _out_a, exit_a) = sink();
    let id = rt.spawn(spec("/bin/sh", &["-c", "echo bye"]), sink_a).unwrap();
    assert_eq!(wait_exit(&exit_a), 0);
    wait_status(rt, &id, |s| s == SessionStatus::Exited(0));

    let (sink_b, out_b, exit_b) = sink();
    rt.attach(&id, sink_b).unwrap();
    wait_for(&out_b, |seen| contains(seen, b"bye"));
    assert_eq!(wait_exit(&exit_b), 0);
}

/// PRD security invariant: PTY output is hostile on every Runtime. The
/// runtime must deliver escape sequences verbatim as data (so tests and
/// renderers can see exactly what the agent sent) and must never act on
/// them; there is deliberately no sequence parser anywhere in the module.
/// The payload covers an OSC 7 cwd report (the CVE class fixed upstream),
/// an OSC 52 clipboard write, a full terminal reset, and a title change.
pub(crate) fn case_hostile_escape_sequences_are_data(rt: &dyn Runtime) {
    let (sink, out, _exit) = sink();
    let hostile = concat!(
        "\x1b]7;file:///etc/passwd\x07",
        "\x1b]52;c;aGVsbXNtZW4=\x07",
        "\x1bc",
        "\x1b]0;owned\x07",
        "MARKER-END"
    );
    let id = rt
        .spawn(spec("/bin/sh", &["-c", &format!("printf '%s' '{hostile}'")]), sink)
        .unwrap();
    let seen = wait_for(&out, |seen| contains(seen, b"MARKER-END"));
    assert!(contains(&seen, b"\x1b]7;file:///etc/passwd\x07"));
    assert!(contains(&seen, b"\x1b]52;c;aGVsbXNtZW4=\x07"));
    assert!(contains(&seen, b"\x1bc"));
    let _ = rt.kill(&id);
}

/// Boundary validation on the spec itself.
pub(crate) fn case_invalid_specs_are_rejected(rt: &dyn Runtime) {
    let empty_program = spec("", &[]);
    let (s1, _o1, _e1) = sink();
    assert!(rt.spawn(empty_program, s1).is_err());

    let mut zero_size = spec("/bin/sh", &["-c", "true"]);
    zero_size.cols = 0;
    let (s2, _o2, _e2) = sink();
    assert!(rt.spawn(zero_size, s2).is_err());

    let mut bad_cwd = spec("/bin/sh", &["-c", "true"]);
    bad_cwd.cwd = "/definitely/not/a/real/dir".to_string();
    let (s3, _o3, _e3) = sink();
    assert!(rt.spawn(bad_cwd, s3).is_err());
}

// --- LocalPty runs the whole suite ---

mod local_pty {
    use super::super::local_pty::LocalPty;

    macro_rules! conformance {
        ($($name:ident),* $(,)?) => {
            $(
                #[test]
                fn $name() {
                    super::$name(&LocalPty::default());
                }
            )*
        };
    }

    conformance!(
        case_spawn_streams_output_with_env,
        case_write_reaches_stdin,
        case_resize_is_observed,
        case_status_reports_exit_code,
        case_kill_terminates,
        case_unknown_session_ids_error,
        case_attach_replays_scrollback_then_streams,
        case_attach_after_exit_replays_and_reports_exit,
        case_hostile_escape_sequences_are_data,
        case_invalid_specs_are_rejected,
    );
}
