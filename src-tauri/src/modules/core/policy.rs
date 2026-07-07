//! Approval policy (M3.5, task #17) — the pure, user-level trust predicate.
//!
//! This is the DECISION SEAM of the Approval Inbox, expressed as `data in ->
//! data out`: given a tool name, its (already-parsed) input, and a trusted
//! [`PolicyContext`], [`decide`] returns one of [`Decision::Allow`],
//! [`Decision::Ask`] (a risk-list rule fired — pause and surface an ask
//! block), or [`Decision::Deny`] (a hard-deny rule fired — the call must
//! never run). Nothing here performs a side effect: it cannot read a file,
//! spawn a process, or touch the network even if it wanted to. That is the
//! strongest form of the PRD invariant "an event may change state but never
//! executes anything" — and it is why the whole risk list is unit-tested
//! against synthetic `(tool, input)` cases and the real spike corpus.
//!
//! # A repo can never configure its own trust
//!
//! The policy lives here, in the user-level pure core — never in the hook
//! script, never read from any repo-committed file. The imperative shell
//! ([`super::super::hooks`]) supplies the [`PolicyContext`] (the trusted
//! workspace root and home directory, known at cut time, not taken from the
//! hostile payload) and enforces the returned [`Decision`] via the Claude
//! Code hook return. A repo cannot loosen or bypass any of this.
//!
//! # The day-1 risk list (all four categories, as pure predicates)
//!
//! 1. **git history rewrites** — force push, rebase, `reset --hard`,
//!    `commit --amend`, `filter-branch` / `filter-repo`.
//! 2. **secrets-adjacent read/writes** — `.env*`, credentials, keychain,
//!    `.netrc` paths (via a file tool's `file_path` or a shell command).
//! 3. **destructive fs OUTSIDE the worktree only** — `rm`/`mv`/… whose target
//!    escapes the workspace root. In-tree destructive ops stay FREE (worktree
//!    isolation pays for this); `..` escapes are rejected.
//! 4. **publish / deploy / DB** — `npm`/`cargo publish`, `docker push`,
//!    `kubectl`, `terraform apply`/`destroy`, DB drop/migrate.
//!
//! Behind the risk list sits a HARD-DENY list that never asks: `sudo`
//! (escalation), `rm` on `$HOME` (catastrophic), and any `~/.ssh` access
//! (private keys). Hard-deny is checked first and, in the shell, is enforced
//! by the hook return so it is robust regardless of the agent's prompt
//! layout.
//!
//! # Structural screening (task #31)
//!
//! The screen is not a shell, so shell STRUCTURE must never hide a command
//! from it. Segmentation therefore breaks on every command-running operator —
//! `;`, `|`/`||`, `&`/`&&`, backticks, and parentheses (`$(…)`, `<(…)`,
//! subshells) — and each segment is screened on the program it actually runs
//! (transparent wrappers such as `env`, `nohup`, `eval`, `xargs`, `bash -c`
//! are stripped first). Output redirections are writes: a `>`/`>>` target
//! outside the worktree asks like any other destructive op. A `cd` to
//! anywhere not provably inside the worktree taints every later relative
//! destructive target (unknown cwd asks, never allows). Where the screen
//! over-approximates — quoted text split at an operator, an unresolvable
//! target — the error is always toward a stricter verdict, never a looser
//! one.

use serde::Serialize;

/// The already-parsed subset of a tool call's input the policy reasons over.
/// The hooks shell extracts these from the (hostile, size-capped, typed-
/// parsed) hook payload; this struct never sees raw JSON. Serialize-only so
/// the same value can ride along on the rendered ask block and the approval
/// record (the exact command / file the decision was made on).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInput {
    /// A shell tool's command line (`Bash`), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// A file tool's target path (`Read` / `Write` / `Edit`), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

impl ToolInput {
    /// A shell command input (test/shell convenience).
    pub fn command(command: impl Into<String>) -> Self {
        Self {
            command: Some(command.into()),
            file_path: None,
        }
    }

    /// A file-path input (test/shell convenience).
    pub fn file_path(path: impl Into<String>) -> Self {
        Self {
            command: None,
            file_path: Some(path.into()),
        }
    }
}

/// The TRUSTED context a decision is made in. Supplied by the shell from what
/// it knew at cut time — never from the hook payload, so a hostile `cwd` in a
/// payload can never redraw the worktree boundary. Empty fields mean "not
/// known": the predicates then fail safe (an unresolvable absolute or
/// home-rooted target is treated as OUTSIDE the worktree — an ask — rather
/// than assumed in-tree).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyContext {
    /// Absolute path of the Workspace's worktree root. In-tree destructive
    /// ops are free relative to this.
    pub workspace_root: String,
    /// Absolute path of the user's home directory (`$HOME`). Anchors the
    /// hard-deny `rm $HOME` and `~/.ssh` checks and tilde expansion.
    pub home_dir: String,
}

impl PolicyContext {
    /// Build a context for a known workspace root and home.
    pub fn new(workspace_root: impl Into<String>, home_dir: impl Into<String>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            home_dir: home_dir.into(),
        }
    }
}

/// A risk-list rule — the call PAUSES with an ask block. Stable [`id`] for
/// records; human [`label`] for the ask block.
///
/// [`id`]: RiskRule::id
/// [`label`]: RiskRule::label
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RiskRule {
    GitHistoryRewrite,
    SecretsAdjacent,
    DestructiveOutsideWorktree,
    PublishDeployDb,
}

impl RiskRule {
    /// Stable machine id (kebab-case) for approval records.
    pub fn id(self) -> &'static str {
        match self {
            RiskRule::GitHistoryRewrite => "git-history-rewrite",
            RiskRule::SecretsAdjacent => "secrets-adjacent",
            RiskRule::DestructiveOutsideWorktree => "destructive-outside-worktree",
            RiskRule::PublishDeployDb => "publish-deploy-db",
        }
    }

    /// Human label for the ask block.
    pub fn label(self) -> &'static str {
        match self {
            RiskRule::GitHistoryRewrite => "git history rewrite",
            RiskRule::SecretsAdjacent => "secrets-adjacent path",
            RiskRule::DestructiveOutsideWorktree => "destructive fs outside the worktree",
            RiskRule::PublishDeployDb => "publish / deploy / database",
        }
    }
}

/// A hard-deny rule — the call NEVER runs and is never asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DenyRule {
    Sudo,
    RmHome,
    SshAccess,
}

impl DenyRule {
    /// Stable machine id (kebab-case) for approval records.
    pub fn id(self) -> &'static str {
        match self {
            DenyRule::Sudo => "hard-deny-sudo",
            DenyRule::RmHome => "hard-deny-rm-home",
            DenyRule::SshAccess => "hard-deny-ssh",
        }
    }

    /// Human label for the record / any surfaced notice.
    pub fn label(self) -> &'static str {
        match self {
            DenyRule::Sudo => "sudo escalation (hard-denied)",
            DenyRule::RmHome => "rm on home directory (hard-denied)",
            DenyRule::SshAccess => "~/.ssh access (hard-denied)",
        }
    }
}

/// What the policy decided for one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Permitted — the agent runs it freely (permissive in-worktree).
    Allow,
    /// A risk-list rule fired: pause and surface an ask block.
    Ask(RiskRule),
    /// A hard-deny rule fired: the call must never run.
    Deny(DenyRule),
}

impl Decision {
    /// Stable rule id, if a rule fired.
    pub fn rule_id(self) -> Option<&'static str> {
        match self {
            Decision::Allow => None,
            Decision::Ask(r) => Some(r.id()),
            Decision::Deny(r) => Some(r.id()),
        }
    }

    /// Human rule label, if a rule fired.
    pub fn rule_label(self) -> Option<&'static str> {
        match self {
            Decision::Allow => None,
            Decision::Ask(r) => Some(r.label()),
            Decision::Deny(r) => Some(r.label()),
        }
    }
}

/// The one policy entry point. Pure and total: `(tool, input, ctx) ->
/// Decision`. Precedence is security-first — the hard-deny list is checked
/// before the risk list, so a `sudo`/`rm $HOME`/`~/.ssh` call is denied even
/// if it would also match a softer risk rule.
pub fn decide(_tool_name: &str, input: &ToolInput, ctx: &PolicyContext) -> Decision {
    // 1. HARD-DENY (never asks). Command-form escalation / catastrophe first.
    if let Some(cmd) = input.command.as_deref() {
        if is_sudo(cmd) {
            return Decision::Deny(DenyRule::Sudo);
        }
        if is_rm_home(cmd, &ctx.home_dir) {
            return Decision::Deny(DenyRule::RmHome);
        }
    }
    // ~/.ssh, via a file tool or a command, is hard-denied outright: reads are
    // the driving case, and splitting read-vs-write out of a shell string is a
    // fragile parser we deliberately avoid — denying all ~/.ssh access is the
    // strictly safer superset.
    if references_ssh(input, &ctx.home_dir) {
        return Decision::Deny(DenyRule::SshAccess);
    }

    // 2. RISK LIST (asks), in the PRD's category order.
    if let Some(cmd) = input.command.as_deref() {
        if is_git_history_rewrite(cmd) {
            return Decision::Ask(RiskRule::GitHistoryRewrite);
        }
    }
    if references_secret(input) {
        return Decision::Ask(RiskRule::SecretsAdjacent);
    }
    if let Some(cmd) = input.command.as_deref() {
        if is_destructive_outside(cmd, &ctx.workspace_root, &ctx.home_dir) {
            return Decision::Ask(RiskRule::DestructiveOutsideWorktree);
        }
        if is_publish_deploy_db(cmd) {
            return Decision::Ask(RiskRule::PublishDeployDb);
        }
    }

    Decision::Allow
}

// ── command tokenizing ──────────────────────────────────────────────────

/// Split a command line into the segments a shell would run, breaking on
/// `;`, newline, `|`/`||`, `&`/`&&` (a lone `&` backgrounds the left side —
/// it separates commands exactly like `;`), backticks, and parentheses
/// (subshells and `$(…)`/`<(…)` substitutions run their contents). Char-safe
/// (never slices a multibyte boundary). This is intentionally coarse — it is
/// a risk screen, not a shell — and it deliberately over-splits, even inside
/// quotes: a dangerous program can never hide behind an operator, at the
/// cost of occasionally screening quoted text as if it were a command, which
/// can only produce a STRICTER verdict, never a looser one (fail safe).
fn command_segments(cmd: &str) -> Vec<String> {
    // `>|` (noclobber override) writes exactly like `>`; normalize it so the
    // redirect scan sees a plain `>` and the `|` does not read as a pipe.
    let cmd = cmd.replace(">|", "> ");
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ';' | '\n' | '`' | '(' | ')' => out.push(std::mem::take(&mut cur)),
            '&' | '|' => {
                if chars.peek() == Some(&c) {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            other => cur.push(other),
        }
    }
    out.push(cur);
    out
}

/// Is `tok` a leading environment assignment (`VAR=val`) rather than a path
/// or program? Used to skip `FOO=bar sudo …` prefixes.
fn is_env_assignment(tok: &str) -> bool {
    match tok.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// Leading tokens that transparently run whatever follows them (or shell
/// grouping / negation prefixes). Skipped when resolving the program a
/// segment actually runs, so `nohup sudo …`, `bash -c 'sudo …'`, or
/// `{ rm … ; }` cannot hide the real program behind a wrapper.
const TRANSPARENT_WRAPPERS: &[&str] = &[
    "env", "command", "builtin", "exec", "eval", "nohup", "time", "xargs", "sh", "bash", "zsh",
    "dash", "fish", "{", "!",
];

/// Strip leading `VAR=val` assignments, transparent wrappers, and any flag
/// tokens between a wrapper and its command (`env -i sudo …`, `bash -c …`),
/// leaving the program a segment actually runs plus its operands.
fn strip_wrappers<'a, 'b>(toks: &'b [&'a str]) -> &'b [&'a str] {
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        let is_flag = t.len() > 1 && t.starts_with('-');
        if TRANSPARENT_WRAPPERS.contains(&t) || is_env_assignment(t) || is_flag {
            i += 1;
        } else {
            break;
        }
    }
    &toks[i..]
}

/// The program a segment runs: wrappers and assignments stripped, quotes and
/// escaping backslashes removed so `"sudo"` / `\sudo` still read as `sudo`.
fn first_program<'a>(toks: &[&'a str]) -> Option<&'a str> {
    strip_wrappers(toks)
        .first()
        .map(|t| unquote(t).trim_start_matches('\\'))
}

/// The non-flag path arguments of a segment (everything after the program
/// that is not a `-flag`), with `dd`'s `if=`/`of=` values unwrapped.
fn path_args<'a>(toks: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    for t in strip_wrappers(toks).iter().skip(1) {
        if let Some(v) = t.strip_prefix("of=").or_else(|| t.strip_prefix("if=")) {
            out.push(v);
        } else if t.starts_with('-') {
            continue;
        } else {
            out.push(*t);
        }
    }
    out
}

/// Filesystem targets of output redirections (`>` / `>>` / `N>`, attached or
/// spaced) within one segment's tokens. Fd duplications (`2>&1`, `>&2`) have
/// no path target and are skipped.
fn redirect_targets<'a>(toks: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let tok = toks[i];
        if let Some(pos) = tok.rfind('>') {
            let after = &tok[pos + 1..];
            if after.starts_with('&') {
                // fd duplication — no path target
            } else if unquote(after).is_empty() {
                // spaced form (`> file`): the next token is the target
                if let Some(next) = toks.get(i + 1) {
                    if !next.starts_with('<') && !next.contains('>') {
                        out.push(*next);
                        i += 1;
                    }
                }
            } else {
                // attached form (`>file`, `1>>file`)
                out.push(after);
            }
        }
        i += 1;
    }
    out
}

/// Writable device sinks that are safe redirect targets from anywhere.
fn is_dev_sink(path: &str) -> bool {
    matches!(
        unquote(path),
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty"
    )
}

fn unquote(arg: &str) -> &str {
    arg.trim_matches(|c| c == '"' || c == '\'')
}

fn strip_trailing_slashes(s: &str) -> &str {
    let t = s.trim_end_matches('/');
    if t.is_empty() {
        s
    } else {
        t
    }
}

// ── hard-deny predicates ────────────────────────────────────────────────

fn is_sudo(cmd: &str) -> bool {
    command_segments(cmd).iter().any(|seg| {
        let toks: Vec<&str> = seg.split_whitespace().collect();
        matches!(first_program(&toks), Some("sudo") | Some("doas"))
    })
}

fn is_home_root(arg: &str, home: &str) -> bool {
    let a = strip_trailing_slashes(unquote(arg));
    a == "~" || a == "$HOME" || a == "${HOME}" || (!home.is_empty() && a == strip_trailing_slashes(home))
}

fn is_rm_home(cmd: &str, home: &str) -> bool {
    command_segments(cmd).iter().any(|seg| {
        let toks: Vec<&str> = seg.split_whitespace().collect();
        if !matches!(first_program(&toks), Some("rm")) {
            return false;
        }
        path_args(&toks).iter().any(|a| is_home_root(a, home))
    })
}

/// A path (file-tool target or bare command token) that points into `~/.ssh`
/// in any of its writable forms.
fn path_in_ssh(path: &str, home: &str) -> bool {
    let p = unquote(path);
    p == "~/.ssh"
        || p.starts_with("~/.ssh/")
        || p == "$HOME/.ssh"
        || p.starts_with("$HOME/.ssh/")
        || p == "${HOME}/.ssh"
        || p.starts_with("${HOME}/.ssh/")
        || (!home.is_empty() && {
            let anchor = format!("{}/.ssh", strip_trailing_slashes(home));
            p == anchor || p.starts_with(&format!("{anchor}/"))
        })
}

fn references_ssh(input: &ToolInput, home: &str) -> bool {
    if let Some(fp) = input.file_path.as_deref() {
        if path_in_ssh(fp, home) {
            return true;
        }
    }
    if let Some(cmd) = input.command.as_deref() {
        // A command references ~/.ssh if any of its tokens is such a path.
        if command_segments(cmd)
            .iter()
            .flat_map(|seg| seg.split_whitespace())
            .any(|tok| path_in_ssh(tok, home))
        {
            return true;
        }
    }
    false
}

// ── risk-list predicates ────────────────────────────────────────────────

fn is_git_history_rewrite(cmd: &str) -> bool {
    for seg in command_segments(cmd) {
        let toks: Vec<&str> = seg.split_whitespace().collect();
        // Require an actual git invocation in this segment (RTK's `rtk git …`
        // still carries the `git` token, so this stays robust to rewrites).
        if !toks.contains(&"git") {
            continue;
        }
        let has = |s: &str| toks.contains(&s);
        let force_flag = toks.iter().any(|t| *t == "-f" || t.starts_with("--force"));
        // A `+refspec` (`git push origin +main:main`) forces the ref update
        // exactly like `--force` — screen it the same way.
        let plus_refspec = toks.iter().any(|t| {
            let t = unquote(t);
            t.len() > 1 && t.starts_with('+')
        });
        if has("push") && (force_flag || plus_refspec) {
            return true;
        }
        if has("rebase") {
            return true;
        }
        if has("reset") && has("--hard") {
            return true;
        }
        if has("commit") && has("--amend") {
            return true;
        }
        if has("filter-branch") || has("filter-repo") {
            return true;
        }
    }
    false
}

/// A `.env` / `.env.*` filename marker: `.env` followed by end, `.`, `/`, or a
/// non-alphanumeric — so `.env.local` and `/app/.env` match but
/// `.environment` does not.
fn has_dotenv(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut i = 0;
    while let Some(pos) = lower[i..].find(".env") {
        let start = i + pos;
        let after = start + 4;
        let ok = match bytes.get(after) {
            None => true,
            Some(&b) => !(b as char).is_ascii_alphanumeric(),
        };
        if ok {
            return true;
        }
        i = after;
    }
    false
}

fn looks_secret(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    has_dotenv(&lower)
        || lower.contains("credentials")
        || lower.contains("keychain")
        || lower.contains(".netrc")
}

fn references_secret(input: &ToolInput) -> bool {
    if let Some(fp) = input.file_path.as_deref() {
        if looks_secret(fp) {
            return true;
        }
    }
    if let Some(cmd) = input.command.as_deref() {
        if looks_secret(cmd) {
            return true;
        }
    }
    false
}

/// Normalize an absolute path by collapsing `.` and `..` components. Pure
/// string work — no filesystem, no symlink resolution.
fn normalize_abs(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    format!("/{}", stack.join("/"))
}

fn under_root(abs: &str, root: &str) -> bool {
    if root.is_empty() {
        return false;
    }
    let a = normalize_abs(abs);
    let r = strip_trailing_slashes(root);
    a == r || a.starts_with(&format!("{r}/"))
}

/// Does a relative path escape above its base (a `..` that pops past the
/// root)? A relative path with `..` that stays at or below the base is
/// in-tree and does not escape.
fn relative_escapes(path: &str) -> bool {
    let mut depth: i32 = 0;
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            _ => depth += 1,
        }
    }
    false
}

/// Does a single destructive target escape the worktree? Home-rooted and
/// absolute targets are resolved against `root`; a relative target is judged
/// purely on whether its `..`s escape. When `root`/`home` is unknown, an
/// unresolvable absolute or home-rooted target fails safe to "escapes"
/// (an ask), never to "in-tree".
fn escapes_worktree(raw: &str, root: &str, home: &str) -> bool {
    let arg = unquote(raw);

    let home_rooted = arg == "~"
        || arg.starts_with("~/")
        || arg == "$HOME"
        || arg.starts_with("$HOME/")
        || arg == "${HOME}"
        || arg.starts_with("${HOME}/");
    if home_rooted {
        if home.is_empty() {
            return true; // cannot prove in-tree
        }
        let rest = arg
            .trim_start_matches('~')
            .trim_start_matches("$HOME")
            .trim_start_matches("${HOME}")
            .trim_start_matches('/');
        let abs = if rest.is_empty() {
            strip_trailing_slashes(home).to_string()
        } else {
            format!("{}/{}", strip_trailing_slashes(home), rest)
        };
        return !under_root(&abs, root);
    }

    if arg.starts_with('/') {
        if root.is_empty() {
            return true; // cannot prove in-tree
        }
        return !under_root(arg, root);
    }

    // Relative: escapes only if its `..`s pop above the worktree root.
    relative_escapes(arg)
}

/// Is this target anchored (absolute or home-rooted), i.e. independent of
/// the — possibly unknown — current directory?
fn is_anchored(arg: &str) -> bool {
    arg.starts_with('/')
        || arg == "~"
        || arg.starts_with("~/")
        || arg == "$HOME"
        || arg.starts_with("$HOME/")
        || arg == "${HOME}"
        || arg.starts_with("${HOME}/")
}

/// [`escapes_worktree`], additionally treating every relative target as
/// escaping once the segment chain has `cd`'d somewhere not provably inside
/// the worktree. Fail safe: an unknowable cwd yields an ask, never an allow.
fn target_escapes(raw: &str, root: &str, home: &str, cwd_escaped: bool) -> bool {
    if cwd_escaped && !is_anchored(unquote(raw)) {
        return true;
    }
    escapes_worktree(raw, root, home)
}

fn is_destructive_outside(cmd: &str, root: &str, home: &str) -> bool {
    const PROGS: &[&str] = &["rm", "rmdir", "mv", "cp", "shred", "truncate", "dd", "tee"];
    // Tracked across segments: has a `cd`/`pushd` moved the cwd somewhere not
    // provably inside the worktree? Once true, a relative target can no
    // longer be trusted as in-tree; an absolute in-tree `cd` restores trust.
    let mut cwd_escaped = false;
    for seg in command_segments(cmd) {
        let toks: Vec<&str> = seg.split_whitespace().collect();
        // A `>` / `>>` write to a path outside the worktree is destructive
        // regardless of which program produced the bytes.
        if redirect_targets(&toks)
            .iter()
            .any(|t| !is_dev_sink(t) && target_escapes(t, root, home, cwd_escaped))
        {
            return true;
        }
        let Some(prog) = first_program(&toks) else {
            continue;
        };
        if prog == "cd" || prog == "pushd" {
            cwd_escaped = match path_args(&toks).first() {
                // Bare `cd` goes to $HOME; `cd -` is unknowable. Both are
                // outside anything we can prove in-tree.
                None => true,
                Some(t) => target_escapes(t, root, home, cwd_escaped),
            };
            continue;
        }
        if !PROGS.contains(&prog) {
            continue;
        }
        if path_args(&toks)
            .iter()
            .any(|a| target_escapes(a, root, home, cwd_escaped))
        {
            return true;
        }
    }
    false
}

fn is_publish_deploy_db(cmd: &str) -> bool {
    for seg in command_segments(cmd) {
        let toks: Vec<&str> = seg.split_whitespace().collect();
        let has = |s: &str| toks.contains(&s);
        match first_program(&toks) {
            Some("npm" | "pnpm" | "yarn" | "bun") if has("publish") => return true,
            Some("cargo") if has("publish") => return true,
            Some("docker") if has("push") => return true,
            Some("kubectl") => return true,
            Some("terraform") if has("apply") || has("destroy") => return true,
            _ => {}
        }
    }
    // DB drop/migrate — matched across the whole command, case-insensitively.
    let lower = cmd.to_ascii_lowercase();
    lower.contains("drop database")
        || lower.contains("drop table")
        || lower.contains("db:migrate")
        || lower.contains("migrate deploy")
        || lower.contains("prisma migrate")
        || lower.contains("alembic upgrade")
        || lower.contains("flyway migrate")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/Users/dev/wt/feature";
    const HOME: &str = "/Users/dev";

    fn ctx() -> PolicyContext {
        PolicyContext::new(ROOT, HOME)
    }

    fn bash(cmd: &str) -> ToolInput {
        ToolInput::command(cmd)
    }

    fn decide_bash(cmd: &str) -> Decision {
        decide("Bash", &bash(cmd), &ctx())
    }

    // ── hard-deny: never asks, checked before the risk list ──────────────

    #[test]
    fn sudo_is_hard_denied() {
        assert_eq!(decide_bash("sudo rm -rf /var"), Decision::Deny(DenyRule::Sudo));
        assert_eq!(
            decide_bash("echo hi && sudo systemctl restart nginx"),
            Decision::Deny(DenyRule::Sudo)
        );
        assert_eq!(
            decide_bash("FOO=bar sudo apt install x"),
            Decision::Deny(DenyRule::Sudo)
        );
        // Not sudo just because the word appears as an argument.
        assert_eq!(decide_bash("echo sudo"), Decision::Allow);
    }

    #[test]
    fn rm_on_home_is_hard_denied_even_before_secrets_or_destructive() {
        for cmd in [
            "rm -rf ~",
            "rm -rf $HOME",
            "rm -rf ${HOME}/",
            "rm -rf /Users/dev",
            "rm -rf /Users/dev/",
        ] {
            assert_eq!(
                decide_bash(cmd),
                Decision::Deny(DenyRule::RmHome),
                "{cmd:?} must be hard-denied"
            );
        }
        // rm of something *inside* home but not home itself is not this rule.
        assert_ne!(
            decide_bash("rm -rf ~/scratch"),
            Decision::Deny(DenyRule::RmHome)
        );
    }

    #[test]
    fn ssh_access_is_hard_denied_via_command_and_file_tool() {
        assert_eq!(
            decide_bash("cat ~/.ssh/id_rsa"),
            Decision::Deny(DenyRule::SshAccess)
        );
        assert_eq!(
            decide_bash("cp $HOME/.ssh/id_ed25519 /tmp/x"),
            Decision::Deny(DenyRule::SshAccess)
        );
        assert_eq!(
            decide("Read", &ToolInput::file_path("/Users/dev/.ssh/id_rsa"), &ctx()),
            Decision::Deny(DenyRule::SshAccess)
        );
        // A ".ssh" that is NOT under home is not this rule.
        assert_ne!(
            decide("Read", &ToolInput::file_path("/app/.ssh/config"), &ctx()),
            Decision::Deny(DenyRule::SshAccess)
        );
    }

    // ── risk 1: git history rewrites ─────────────────────────────────────

    #[test]
    fn git_history_rewrites_ask() {
        for cmd in [
            "git push --force",
            "git push -f origin main",
            "git push --force-with-lease",
            "git rebase -i HEAD~3",
            "git reset --hard HEAD~1",
            "git commit --amend -m x",
            "git filter-branch --tree-filter x",
            "git filter-repo --path secret",
        ] {
            assert_eq!(
                decide_bash(cmd),
                Decision::Ask(RiskRule::GitHistoryRewrite),
                "{cmd:?} is a history rewrite"
            );
        }
        // Non-rewrite git stays free.
        assert_eq!(decide_bash("git status"), Decision::Allow);
        assert_eq!(decide_bash("git push origin main"), Decision::Allow);
        assert_eq!(decide_bash("git log --oneline -3"), Decision::Allow);
    }

    #[test]
    fn rtk_rewritten_git_still_matches_because_the_git_token_survives() {
        assert_eq!(
            decide_bash("rtk git push --force"),
            Decision::Ask(RiskRule::GitHistoryRewrite)
        );
    }

    // ── risk 2: secrets-adjacent ─────────────────────────────────────────

    #[test]
    fn secrets_adjacent_paths_ask() {
        assert_eq!(
            decide_bash("cat .env"),
            Decision::Ask(RiskRule::SecretsAdjacent)
        );
        assert_eq!(
            decide_bash("cp .env.local /tmp/x"),
            Decision::Ask(RiskRule::SecretsAdjacent)
        );
        assert_eq!(
            decide("Write", &ToolInput::file_path("config/.env.production"), &ctx()),
            Decision::Ask(RiskRule::SecretsAdjacent)
        );
        assert_eq!(
            decide("Read", &ToolInput::file_path("/Users/dev/.aws/credentials"), &ctx()),
            Decision::Ask(RiskRule::SecretsAdjacent)
        );
        assert_eq!(
            decide_bash("cat ~/.netrc"),
            Decision::Ask(RiskRule::SecretsAdjacent)
        );
    }

    #[test]
    fn environment_word_is_not_a_dotenv_false_positive() {
        assert_eq!(decide_bash("cat environment.md"), Decision::Allow);
        assert_eq!(
            decide("Read", &ToolInput::file_path("docs/.environment-notes"), &ctx()),
            Decision::Allow
        );
    }

    // ── risk 3: destructive fs outside the worktree only ─────────────────

    #[test]
    fn in_tree_destructive_ops_are_free() {
        for cmd in [
            "rm -rf node_modules",
            "rm build/output.js",
            "rm ./dist/app.js",
            "mv src/a.rs src/b.rs",
            "rm -rf sub/dir/../other", // .. that stays in-tree
        ] {
            assert_eq!(decide_bash(cmd), Decision::Allow, "{cmd:?} is in-tree, free");
        }
    }

    #[test]
    fn destructive_ops_outside_the_worktree_ask() {
        for cmd in [
            "rm -rf /etc/hosts",
            "rm ../sibling/file",
            "rm -rf ../../other-repo",
            "mv secret.txt /tmp/exfil",
            "rm /Users/dev/wt/feature/../escape", // .. escapes the root
        ] {
            assert_eq!(
                decide_bash(cmd),
                Decision::Ask(RiskRule::DestructiveOutsideWorktree),
                "{cmd:?} escapes the worktree"
            );
        }
    }

    #[test]
    fn absolute_targets_inside_the_root_are_free() {
        assert_eq!(
            decide_bash("rm -rf /Users/dev/wt/feature/target"),
            Decision::Allow
        );
    }

    #[test]
    fn unknown_root_fails_safe_to_ask_for_absolute_targets() {
        let ctx = PolicyContext::new("", HOME);
        assert_eq!(
            decide("Bash", &bash("rm /var/tmp/x"), &ctx),
            Decision::Ask(RiskRule::DestructiveOutsideWorktree)
        );
        // …but a plainly in-tree relative target is still free.
        assert_eq!(decide("Bash", &bash("rm build/x"), &ctx), Decision::Allow);
    }

    // ── risk 4: publish / deploy / DB ────────────────────────────────────

    #[test]
    fn publish_deploy_db_ask() {
        for cmd in [
            "npm publish",
            "pnpm publish --access public",
            "cargo publish",
            "docker push registry/app:latest",
            "kubectl apply -f deploy.yaml",
            "terraform apply -auto-approve",
            "terraform destroy",
            "psql -c 'DROP TABLE users'",
            "rails db:migrate",
            "prisma migrate deploy",
        ] {
            assert_eq!(
                decide_bash(cmd),
                Decision::Ask(RiskRule::PublishDeployDb),
                "{cmd:?} is publish/deploy/db"
            );
        }
        // A local dry-run build stays free.
        assert_eq!(decide_bash("cargo build"), Decision::Allow);
        assert_eq!(decide_bash("npm run test"), Decision::Allow);
    }

    // ── precedence + defaults ────────────────────────────────────────────

    #[test]
    fn hard_deny_beats_a_softer_risk_rule() {
        // `sudo` on a publish command is still hard-denied, not merely asked.
        assert_eq!(
            decide_bash("sudo docker push registry/app"),
            Decision::Deny(DenyRule::Sudo)
        );
    }

    #[test]
    fn ordinary_calls_are_allowed() {
        for cmd in ["ls -la", "cat README.md", "git status", "cargo test", "echo hi"] {
            assert_eq!(decide_bash(cmd), Decision::Allow, "{cmd:?} is ordinary");
        }
        assert_eq!(decide("Bash", &ToolInput::default(), &ctx()), Decision::Allow);
    }

    #[test]
    fn rule_ids_and_labels_are_stable() {
        assert_eq!(RiskRule::GitHistoryRewrite.id(), "git-history-rewrite");
        assert_eq!(DenyRule::Sudo.id(), "hard-deny-sudo");
        assert_eq!(
            Decision::Ask(RiskRule::PublishDeployDb).rule_id(),
            Some("publish-deploy-db")
        );
        assert_eq!(Decision::Allow.rule_id(), None);
        assert!(Decision::Deny(DenyRule::SshAccess)
            .rule_label()
            .unwrap()
            .contains(".ssh"));
    }

    // ── #31: structural bypass vectors must never land on Allow ──────────

    #[test]
    fn background_operator_does_not_hide_sudo() {
        // A lone `&` is a segment separator: the sudo after it is screened.
        assert_eq!(
            decide_bash("echo hi & sudo rm -rf /etc"),
            Decision::Deny(DenyRule::Sudo)
        );
        // A backgrounded destructive op is screened like any other segment.
        assert_eq!(
            decide_bash("rm -rf /etc & echo done"),
            Decision::Ask(RiskRule::DestructiveOutsideWorktree)
        );
    }

    #[test]
    fn redirection_writes_outside_the_worktree_ask() {
        for cmd in [
            "echo pwned > /etc/cron.d/backdoor",
            "echo pwned >/etc/cron.d/backdoor",
            "echo pwned >> ~/.zprofile",
            "printf x 1> /etc/motd",
            "echo x >| /etc/motd",
        ] {
            assert_eq!(
                decide_bash(cmd),
                Decision::Ask(RiskRule::DestructiveOutsideWorktree),
                "{cmd:?} writes outside the worktree"
            );
        }
        // In-tree and device-sink redirects stay free.
        assert_eq!(decide_bash("echo hi > out.txt"), Decision::Allow);
        assert_eq!(decide_bash("cargo test > /dev/null 2>&1"), Decision::Allow);
        assert_eq!(decide_bash("git status 2>&1"), Decision::Allow);
        assert_eq!(
            decide_bash("rg TODO src > notes/todo.txt"),
            Decision::Allow
        );
    }

    #[test]
    fn cd_out_of_the_worktree_taints_relative_destructive_targets() {
        for cmd in [
            "cd / && rm -rf etc",
            "cd /tmp; rm -rf sessions",
            "cd .. && rm -rf other-checkout",
            "cd && rm -rf .config",
        ] {
            assert_eq!(
                decide_bash(cmd),
                Decision::Ask(RiskRule::DestructiveOutsideWorktree),
                "{cmd:?} deletes outside the worktree after a cd"
            );
        }
        // cd within the worktree keeps in-tree destructive ops free.
        assert_eq!(decide_bash("cd src && rm -rf build"), Decision::Allow);
        assert_eq!(
            decide_bash("cd /Users/dev/wt/feature/sub && rm -rf node_modules"),
            Decision::Allow
        );
        // Returning into the worktree by absolute path restores trust.
        assert_eq!(
            decide_bash("cd /tmp && cd /Users/dev/wt/feature && rm -rf dist"),
            Decision::Allow
        );
    }

    #[test]
    fn plus_refspec_force_push_asks() {
        for cmd in [
            "git push origin +main:main",
            "git push origin +refs/heads/main",
            "rtk git push origin +main",
        ] {
            assert_eq!(
                decide_bash(cmd),
                Decision::Ask(RiskRule::GitHistoryRewrite),
                "{cmd:?} force-pushes via a +refspec"
            );
        }
        // Plain fast-forward pushes stay free.
        assert_eq!(decide_bash("git push origin main"), Decision::Allow);
        assert_eq!(decide_bash("git push origin main:main"), Decision::Allow);
    }

    #[test]
    fn substitution_and_subshells_do_not_hide_commands() {
        assert_eq!(
            decide_bash("x=$(sudo cat /etc/shadow)"),
            Decision::Deny(DenyRule::Sudo)
        );
        assert_eq!(
            decide_bash("x=`sudo cat /etc/shadow`"),
            Decision::Deny(DenyRule::Sudo)
        );
        assert_eq!(
            decide_bash("(cd / && rm -rf etc)"),
            Decision::Ask(RiskRule::DestructiveOutsideWorktree)
        );
        assert_eq!(
            decide_bash("cat <(sudo ls /root)"),
            Decision::Deny(DenyRule::Sudo)
        );
        // Benign substitutions stay free.
        assert_eq!(decide_bash("echo $(date)"), Decision::Allow);
        assert_eq!(
            decide_bash("VERSION=$(git describe) cargo build"),
            Decision::Allow
        );
    }

    #[test]
    fn transparent_wrappers_do_not_hide_sudo() {
        for cmd in [
            "nohup sudo shutdown -h now",
            "eval sudo reboot",
            "bash -c 'sudo rm -rf /'",
            "xargs sudo rm",
        ] {
            assert_eq!(
                decide_bash(cmd),
                Decision::Deny(DenyRule::Sudo),
                "{cmd:?} runs sudo through a transparent wrapper"
            );
        }
    }

    #[test]
    fn tee_outside_the_worktree_asks() {
        assert_eq!(
            decide_bash("echo 1 | tee /etc/hosts"),
            Decision::Ask(RiskRule::DestructiveOutsideWorktree)
        );
        assert_eq!(decide_bash("pnpm test | tee test.log"), Decision::Allow);
    }

    // ── the spike corpus: every captured PreToolUse is an ordinary git
    //    read, so the policy allows all of them (no ask, no deny) ──────────

    #[test]
    fn spike_corpus_commands_are_all_allowed() {
        for cmd in [
            "git log --oneline -3",
            "git status",
            "git status",
            "git diff --stat",
        ] {
            assert_eq!(decide_bash(cmd), Decision::Allow, "spike command {cmd:?}");
        }
    }
}
