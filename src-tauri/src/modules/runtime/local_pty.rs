//! LocalPty: the M1 Runtime. A real PTY on this machine via portable-pty
//! (the same crate Terax's own terminal uses); sessions die with the app.
//!
//! Bytes are moved verbatim in both directions on the LIVE paths (the
//! streaming sink and `attach` backfill): there is no escape-sequence parser
//! there on purpose — output is hostile, and interpreting it for display is
//! exclusively the (sandboxed) renderer's job.
//!
//! The ONE exception is [`snapshot`](Runtime::snapshot), the read-only
//! `capture-pane` analog. It reconstructs the CURRENT VISIBLE SCREEN from the
//! retained scrollback through a terminal-grid model (`vt100`) — exactly what
//! Tmux's `capture-pane` does natively at M4 — so the verify-before-inject
//! answering seam matches the live dialog, not stale history (user story 30).
//! Its output never re-enters the UI stream; it feeds only the safety check.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

use super::{OutputSink, Runtime, SessionStatus, SpawnSpec};

const READ_BUF: usize = 16 * 1024;
/// Retained per session for attach backfill. Trimming may slice an escape
/// sequence at the front; acceptable for a backfill buffer, and the live
/// stream is never trimmed.
const SCROLLBACK_MAX: usize = 256 * 1024;

/// Sink + status share one lock so an attach can never race the exit
/// notification: whoever holds the lock decides which sink learns of the
/// exit, and each sink hears it at most once.
struct Ctl {
    sink: Arc<OutputSink>,
    status: SessionStatus,
    scrollback: Vec<u8>,
    /// Current pane size, tracked from spawn and every `resize`. `snapshot`
    /// replays the scrollback into a grid of exactly these dimensions so the
    /// reconstructed screen matches what the agent's TUI actually rendered.
    cols: u16,
    rows: u16,
}

struct Session {
    ctl: Mutex<Ctl>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

impl Session {
    /// Signal the child, but only while it is still running. `status` flips
    /// to `Exited` only after `child.wait()` returned — i.e. after the OS
    /// reaped the pid — so an exited status means the pid may already belong
    /// to an unrelated process and must NOT be signalled (fail safe: skip),
    /// while a running status means the pid is still ours (alive or an
    /// unreaped zombie, which ignores signals harmlessly).
    ///
    /// Locks are read through poison on purpose: teardown must still reap a
    /// child even if a panic poisoned a lock, and both protected values stay
    /// coherent for this use (status is a plain flag; the killer is a pid
    /// handle).
    fn kill_if_running(&self) -> Result<(), String> {
        let status = self.ctl.lock().unwrap_or_else(PoisonError::into_inner).status;
        if matches!(status, SessionStatus::Exited(_)) {
            return Ok(());
        }
        self.killer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .kill()
            .map_err(|e| e.to_string())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Last resort: a session dropped without an explicit kill must not
        // leak its child. NOTE this alone cannot reap a LIVE child at app
        // teardown — the reader/waiter threads hold `Arc<Session>` clones
        // until the child exits, so the refcount reaches zero only after
        // exit. Live children are reaped by `LocalPty::shutdown` instead,
        // which kills through the shared handle at any refcount.
        let _ = self.kill_if_running();
    }
}

#[derive(Default)]
pub struct LocalPty {
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    next_id: AtomicU64,
}

impl LocalPty {
    fn get(&self, session: &str) -> Result<Arc<Session>, String> {
        self.sessions
            .read()
            .expect("sessions lock poisoned")
            .get(session)
            .cloned()
            .ok_or_else(|| format!("no session {session:?}"))
    }

    /// Explicit teardown: kill every live child and forget all sessions.
    /// Also runs on `Drop`, so the "sessions die with the app" contract
    /// holds for graceful shutdown. Killing goes through each session's
    /// shared killer handle — it deliberately does NOT rely on `Session`'s
    /// own `Drop`, whose refcount cannot reach zero while the child lives
    /// (the reader/waiter threads hold clones). Idempotent, and proceeds
    /// through poisoned locks: teardown must reap children even mid-panic.
    pub fn shutdown(&self) {
        let sessions: Vec<Arc<Session>> = {
            let mut map = self
                .sessions
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            map.drain().map(|(_, session)| session).collect()
        };
        for session in sessions {
            // Best effort per child: one failed signal must not stop the
            // rest of the teardown.
            if let Err(e) = session.kill_if_running() {
                log::warn!("local-pty shutdown: failed to kill child: {e}");
            }
        }
    }

    /// Drop every exited session from the map, releasing its retained
    /// scrollback (up to [`SCROLLBACK_MAX`]); returns how many were pruned.
    /// Running sessions are untouched. A pruned id then fails every
    /// operation exactly like an unknown id — callers decide when an exited
    /// session's backlog is no longer needed (the post-exit `status`/`attach`
    /// contract holds until they do).
    pub fn prune_exited(&self) -> usize {
        let mut map = self.sessions.write().expect("sessions lock poisoned");
        let before = map.len();
        map.retain(|_, session| {
            // Read through poison: one session's poisoned lock must not
            // panic the sweep (which would poison the map lock for everyone).
            matches!(
                session
                    .ctl
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .status,
                SessionStatus::Running
            )
        });
        before - map.len()
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        // App teardown (task #35): live children are reaped, never orphaned.
        self.shutdown();
    }
}

fn validate_spec(spec: &SpawnSpec) -> Result<(), String> {
    if spec.program.trim().is_empty() {
        return Err("spawn: program must not be empty".to_string());
    }
    if spec.cols == 0 || spec.rows == 0 {
        return Err("spawn: cols and rows must be non-zero".to_string());
    }
    if !Path::new(&spec.cwd).is_absolute() || !Path::new(&spec.cwd).is_dir() {
        return Err(format!("spawn: cwd {:?} is not an absolute directory", spec.cwd));
    }
    Ok(())
}

impl Runtime for LocalPty {
    fn spawn(&self, spec: SpawnSpec, sink: OutputSink) -> Result<String, String> {
        validate_spec(&spec)?;

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;

        // Argv, not a shell line: nothing in the spec gets re-parsed.
        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }

        let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);

        let mut killer = child.clone_killer();
        // Kill the child if pipe setup fails so it can't outlive an
        // aborted spawn.
        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(e) => {
                let _ = killer.kill();
                return Err(e.to_string());
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(e) => {
                let _ = killer.kill();
                return Err(e.to_string());
            }
        };

        let id = format!("lpty-{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let session = Arc::new(Session {
            ctl: Mutex::new(Ctl {
                sink: Arc::new(sink),
                status: SessionStatus::Running,
                scrollback: Vec::with_capacity(READ_BUF),
                cols: spec.cols,
                rows: spec.rows,
            }),
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
        });
        self.sessions
            .write()
            .expect("sessions lock poisoned")
            .insert(id.clone(), session.clone());

        let reader_session = session.clone();
        let reader_thread = thread::Builder::new()
            .name(format!("helm-lpty-reader-{id}"))
            .spawn(move || {
                let mut buf = [0u8; READ_BUF];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let sink = {
                                let mut ctl =
                                    reader_session.ctl.lock().expect("ctl lock poisoned");
                                ctl.scrollback.extend_from_slice(&buf[..n]);
                                let overflow = ctl.scrollback.len().saturating_sub(SCROLLBACK_MAX);
                                if overflow > 0 {
                                    ctl.scrollback.drain(..overflow);
                                }
                                ctl.sink.clone()
                            };
                            (sink.on_output)(&buf[..n]);
                        }
                    }
                }
            })
            .expect("spawn local-pty reader thread");

        let waiter_session = session;
        thread::Builder::new()
            .name(format!("helm-lpty-waiter-{id}"))
            .spawn(move || {
                let code = match child.wait() {
                    Ok(status) => status.exit_code() as i32,
                    Err(e) => {
                        log::warn!("local-pty child wait failed: {e}");
                        -1
                    }
                };
                // Drain the reader first so the final output chunk lands in
                // the scrollback (and the old sink) before anyone hears of
                // the exit.
                if reader_thread.join().is_err() {
                    log::error!("local-pty reader thread panicked");
                }
                let sink = {
                    let mut ctl = waiter_session.ctl.lock().expect("ctl lock poisoned");
                    ctl.status = SessionStatus::Exited(code);
                    ctl.sink.clone()
                };
                (sink.on_exit)(code);
            })
            .expect("spawn local-pty waiter thread");

        Ok(id)
    }

    fn attach(&self, session: &str, sink: OutputSink) -> Result<(), String> {
        let session = self.get(session)?;
        let mut ctl = session.ctl.lock().expect("ctl lock poisoned");
        if !ctl.scrollback.is_empty() {
            (sink.on_output)(&ctl.scrollback);
        }
        if let SessionStatus::Exited(code) = ctl.status {
            // The exit already happened; this sink would otherwise never
            // hear it. The previous sink got its own notification.
            (sink.on_exit)(code);
        }
        ctl.sink = Arc::new(sink);
        Ok(())
    }

    fn write(&self, session: &str, bytes: &[u8]) -> Result<(), String> {
        let session = self.get(session)?;
        let mut writer = session.writer.lock().expect("writer lock poisoned");
        // EPIPE here is expected if the child just exited.
        writer.write_all(bytes).map_err(|e| e.to_string())
    }

    fn snapshot(&self, session: &str) -> Result<Vec<u8>, String> {
        let session = self.get(session)?;
        // Read-only capture-pane: under the same lock that guards the live
        // sink (so a snapshot never races an in-flight chunk), replay the
        // retained scrollback into a terminal grid sized to the current pane
        // and return only the VISIBLE screen. A dialog that was drawn then
        // cleared leaves no trace; a queued dialog rendered on top hides the
        // one beneath — so the answering seam verifies against what is truly
        // on screen now, not session history (user story 30). The live sink
        // is untouched.
        let ctl = session.ctl.lock().expect("ctl lock poisoned");
        let mut grid = vt100::Parser::new(ctl.rows, ctl.cols, 0);
        grid.process(&ctl.scrollback);
        Ok(grid.screen().contents().into_bytes())
    }

    fn resize(&self, session: &str, cols: u16, rows: u16) -> Result<(), String> {
        if cols == 0 || rows == 0 {
            return Err("resize: cols and rows must be non-zero".to_string());
        }
        let session = self.get(session)?;
        // Keep the snapshot grid dimensions in step with the live pane so a
        // reconstructed screen wraps exactly as the agent's TUI did.
        {
            let mut ctl = session.ctl.lock().expect("ctl lock poisoned");
            ctl.cols = cols;
            ctl.rows = rows;
        }
        let master = session.master.lock().expect("master lock poisoned");
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())
    }

    fn status(&self, session: &str) -> Result<SessionStatus, String> {
        let session = self.get(session)?;
        let status = session.ctl.lock().expect("ctl lock poisoned").status;
        Ok(status)
    }

    fn kill(&self, session: &str) -> Result<(), String> {
        // Idempotent on an already-exited session (trait contract): the
        // helper skips the signal once the status says exited.
        self.get(session)?.kill_if_running()
    }
}

/// Session-lifecycle tests (issue #35): teardown must reap live children and
/// the map must be prunable. The behavioral conformance suite (streaming,
/// snapshot, attach) lives in `super::conformance` and is untouched here.
#[cfg(all(test, unix))]
mod lifecycle_tests {
    use std::process::Command;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::{Duration, Instant};

    use super::*;

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

    /// Drain output until `pred` matches the accumulated transcript or the
    /// deadline passes; panics with the transcript on timeout.
    fn wait_for(rx: &Receiver<Vec<u8>>, pred: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let deadline = Instant::now() + DEADLINE;
        let mut seen: Vec<u8> = Vec::new();
        while !pred(&seen) {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(chunk) => seen.extend_from_slice(&chunk),
                Err(_) => panic!(
                    "timeout waiting for output; transcript so far: {:?}",
                    String::from_utf8_lossy(&seen)
                ),
            }
        }
        seen
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len().max(1)).any(|w| w == needle)
    }

    /// Extract the pid the child echoed as `pid:<digits>`.
    fn pid_from(transcript: &[u8]) -> i32 {
        let text = String::from_utf8_lossy(transcript);
        let after = text.split("pid:").nth(1).expect("transcript carries pid:");
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().expect("pid digits parse")
    }

    /// True while `pid` exists in the process table (including as a zombie,
    /// so a killed-but-unreaped child still counts as leaked).
    fn process_exists(pid: i32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn wait_until_gone(pid: i32) {
        let deadline = Instant::now() + DEADLINE;
        while process_exists(pid) {
            assert!(
                Instant::now() < deadline,
                "child {pid} still exists after teardown: orphaned or unreaped"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Spawn a child that outlives any test deadline and report its pid.
    /// `exec` makes the echoed `$$` the pid of the long-lived process itself,
    /// so the liveness probe watches the real child, not a shell wrapper.
    fn spawn_sleeper(rt: &LocalPty) -> (String, i32, Receiver<i32>) {
        let (sink, out, exit) = sink();
        let id = rt
            .spawn(spec("/bin/sh", &["-c", "echo pid:$$; exec sleep 300"]), sink)
            .expect("spawn sleeper");
        let transcript = wait_for(&out, |seen| {
            // Wait for the newline so the pid digits are complete.
            contains(seen, b"pid:") && seen.ends_with(b"\n")
        });
        (id, pid_from(&transcript), exit)
    }

    /// Issue #35 (1): dropping the runtime must reap a LIVE child. The reader
    /// and waiter threads each hold an `Arc<Session>` while the child runs,
    /// so the kill must not depend on the refcount reaching zero — before the
    /// fix this test times out with the child orphaned, still sleeping.
    #[test]
    fn drop_reaps_a_live_child_instead_of_orphaning_it() {
        let rt = LocalPty::default();
        let (id, pid, exit) = spawn_sleeper(&rt);
        assert_eq!(rt.status(&id).unwrap(), SessionStatus::Running);
        assert!(process_exists(pid), "sleeper must be alive before teardown");

        drop(rt);

        // The waiter thread reaps the killed child and fires on_exit long
        // before the child's own 300s runtime — the child did not survive.
        exit.recv_timeout(DEADLINE)
            .expect("teardown must terminate the live child (on_exit never fired)");
        wait_until_gone(pid);
    }

    /// Explicit `shutdown()` (the non-Drop teardown path) reaps live
    /// children the same way and empties the map: afterwards the id no
    /// longer resolves. Calling it again is a no-op.
    #[test]
    fn shutdown_reaps_live_children_and_clears_the_map() {
        let rt = LocalPty::default();
        let (id, pid, exit) = spawn_sleeper(&rt);

        rt.shutdown();

        exit.recv_timeout(DEADLINE)
            .expect("shutdown must terminate the live child (on_exit never fired)");
        wait_until_gone(pid);
        assert!(
            rt.status(&id).is_err(),
            "a torn-down session id must no longer resolve"
        );
        rt.shutdown(); // idempotent
    }

    /// Issue #35 (2): exited sessions must be removable from the map so a
    /// long-lived app does not accumulate scrollback forever. Pruning takes
    /// only exited sessions; their ids then fail like unknown ids, while
    /// running sessions keep resolving.
    #[test]
    fn prune_exited_removes_exited_sessions_and_keeps_running_ones() {
        let rt = LocalPty::default();

        // One session that exits immediately...
        let (done_sink, _done_out, done_exit) = sink();
        let done = rt
            .spawn(spec("/bin/sh", &["-c", "exit 3"]), done_sink)
            .expect("spawn short-lived session");
        assert_eq!(done_exit.recv_timeout(DEADLINE).expect("exit fires"), 3);
        // The waiter flips the status before firing on_exit today, but poll
        // rather than couple the test to that ordering.
        let deadline = Instant::now() + DEADLINE;
        while rt.status(&done).expect("status stays queryable until pruned")
            == SessionStatus::Running
        {
            assert!(Instant::now() < deadline, "status never flipped to exited");
            std::thread::sleep(Duration::from_millis(10));
        }

        // ...and one that is still running.
        let (live, live_pid, _live_exit) = spawn_sleeper(&rt);

        assert_eq!(rt.prune_exited(), 1, "exactly the exited session is pruned");
        assert!(
            rt.status(&done).is_err(),
            "a pruned (zombie) id must stop resolving, like any unknown id"
        );
        assert!(rt.snapshot(&done).is_err(), "pruned id has no snapshot");
        assert_eq!(
            rt.status(&live).expect("running session must survive a prune"),
            SessionStatus::Running
        );
        assert!(process_exists(live_pid), "pruning must not touch live children");
        assert_eq!(rt.prune_exited(), 0, "nothing left to prune");

        rt.kill(&live).expect("cleanup: kill the sleeper");
    }
}
