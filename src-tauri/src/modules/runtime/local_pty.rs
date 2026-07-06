//! LocalPty: the M1 Runtime. A real PTY on this machine via portable-pty
//! (the same crate Terax's own terminal uses); sessions die with the app.
//!
//! Bytes are moved verbatim in both directions. There is no escape-
//! sequence parser here on purpose: output is hostile, and interpreting
//! it is exclusively the (sandboxed) renderer's job.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
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
}

struct Session {
    ctl: Mutex<Ctl>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // A session dropped without an explicit kill (map cleared, app
        // teardown) must not leak its child.
        if let Ok(mut k) = self.killer.lock() {
            let _ = k.kill();
        }
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
        // Read-only: clone the retained scrollback under the same lock that
        // guards the live sink, so a snapshot never races an in-flight chunk.
        // The live sink is untouched — verify-before-inject reads the screen
        // without disturbing the UI's stream. Bind to a local so the
        // MutexGuard temporary drops before `session` (tail-expression drop
        // order).
        let screen = session.ctl.lock().expect("ctl lock poisoned").scrollback.clone();
        Ok(screen)
    }

    fn resize(&self, session: &str, cols: u16, rows: u16) -> Result<(), String> {
        if cols == 0 || rows == 0 {
            return Err("resize: cols and rows must be non-zero".to_string());
        }
        let session = self.get(session)?;
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
        let session = self.get(session)?;
        let already_exited = matches!(
            session.ctl.lock().expect("ctl lock poisoned").status,
            SessionStatus::Exited(_)
        );
        if already_exited {
            return Ok(());
        }
        // Bind to a local so the MutexGuard temporary drops before
        // `session` (tail-expression temporary drop order).
        let result = session
            .killer
            .lock()
            .expect("killer lock poisoned")
            .kill()
            .map_err(|e| e.to_string());
        result
    }
}
