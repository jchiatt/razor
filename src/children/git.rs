//! Git as a program, run to its end in a checkout. At most 64 KiB of each stream is kept; the
//! rest is read and dropped, so git never waits on a full pipe. The names settle moves stay bytes.

use super::{Error, other};
use crate::{protocol::Worktree, shell::Group};
use std::{
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    fs,
    io::{self, AsyncRead, AsyncReadExt},
    process::Command,
};

const KEPT: u64 = 64 * 1024;

struct Git {
    code: Option<i32>,
    out: Vec<u8>,
    text: String,
}

impl Git {
    /// Standard output on success; otherwise a refusal in git's own words.
    fn success(self) -> Result<Vec<u8>, Error> {
        match self.code {
            Some(0) => Ok(self.out),
            _ => Err(Error::Refused(self.text)),
        }
    }
}

fn git(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .env("LC_ALL", "C")
        .process_group(0)
        .stdin(Stdio::null());
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

/// Git run to its end; dropped first, it kills git's process group, so an SSH or credential
/// program git started cannot outlive it.
async fn run(command: &mut Command) -> Result<Git, Error> {
    let mut child = command.spawn().map_err(other("starting git"))?;
    let mut group = Group(child.id());
    let (out, err) = tokio::join!(kept(child.stdout.take()), kept(child.stderr.take()));
    let code = child.wait().await.map_err(other("running git"))?.code();
    group.0 = None;
    let text = String::from_utf8_lossy(&out).into_owned() + &String::from_utf8_lossy(&err);
    Ok(Git { code, out, text })
}

/// Git in `cwd` with `args`, never prompting and stopped after `secs` seconds: its standard
/// output, or a refusal in git's own words.
pub(crate) async fn timed(cwd: &Path, secs: u64, args: &[&str]) -> Result<String, Error> {
    let mut command = git(cwd);
    command.env("GIT_TERMINAL_PROMPT", "0").args(args);
    let ran = tokio::time::timeout(Duration::from_secs(secs), run(&mut command)).await;
    let late = || Error::Refused(format!("Git did not finish within {secs} seconds."));
    let out = ran.map_err(|_| late())??.success()?;
    Ok(String::from_utf8_lossy(&out).trim_end().into())
}

async fn kept(pipe: Option<impl AsyncRead + Unpin>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        // A pipe that fails to read keeps what it gave; git's exit code still decides.
        let _ = (&mut pipe).take(KEPT).read_to_end(&mut bytes).await;
        let _ = io::copy(&mut pipe, &mut io::sink()).await;
    }
    bytes
}

/// The whole names in NUL-ended output; a list cut at KEPT loses only its last, partial name.
fn names(out: &[u8]) -> impl Iterator<Item = &[u8]> {
    out.split_inclusive(|b| *b == 0)
        .filter_map(|name| name.strip_suffix(b"\0"))
}

/// The checkout holding `cwd`, its current commit, and where `cwd` sits inside it; `None`
/// outside git.
pub(super) async fn plan(cwd: &Path) -> Result<Option<(Worktree, PathBuf)>, Error> {
    let found = run(git(cwd).args(["rev-parse", "--show-toplevel", "HEAD"])).await?;
    let out = match found.success() {
        Err(Error::Refused(text)) if text.contains("not a git repository") => return Ok(None),
        out => String::from_utf8_lossy(&out?).into_owned(),
    };
    let (origin, base) = out.trim_end().rsplit_once('\n').unwrap_or_default();
    let inside = cwd
        .strip_prefix(origin)
        .map_err(other("placing the child in its checkout"))?;
    let (origin, base) = (origin.into(), base.into());
    Ok(Some((Worktree { origin, base }, inside.into())))
}

/// A new branch at the worktree's base, checked out at `path`.
pub(super) async fn add(worktree: &Worktree, path: &Path, branch: &str) -> Result<(), Error> {
    let mut add = git(&worktree.origin);
    add.args(["worktree", "add", "-b", branch]);
    run(add.arg(path).arg(&worktree.base))
        .await?
        .success()
        .map(drop)
}

/// The whole names `branch` changed since the worktree's base, or `None` when git cannot say.
pub(super) async fn files(worktree: &Worktree, branch: &str) -> Option<Vec<PathBuf>> {
    let range = format!("{}...{branch}", worktree.base);
    let diff = run(git(&worktree.origin).args(["diff", "--name-only", "-z", &range])).await;
    let out = diff.ok()?.success().ok()?;
    let text = names(&out).map(String::from_utf8_lossy);
    Some(text.map(|n| n.into_owned().into()).collect())
}

/// Merges `branch` into whatever the origin has checked out. A conflict is aborted and refused
/// in git's words; a merge someone else had begun is never touched.
pub(super) async fn merge(origin: &Path, branch: &str) -> Result<String, Error> {
    let merged = run(git(origin).args(["merge", "--no-edit", branch])).await?;
    // Exit 1 is a merge this call began (a conflict) or never began; a merge already in
    // progress makes git exit 128 before it starts.
    if merged.code == Some(1) {
        let aborted = run(git(origin).args(["merge", "--abort"])).await?;
        return Err(Error::Refused(merged.text + &aborted.text));
    }
    Ok(String::from_utf8_lossy(&merged.success()?).into_owned())
}

/// Moves every path `git status` lists in the worktree at `path` (untracked and not ignored, a
/// wholly untracked folder as one path, or changed since its last commit), except deletions, to
/// the same path under `keep`, when the listing is whole. Once each has moved and none is
/// unmerged, resets the tracked files to the commit, then asks git to remove the worktree, which
/// it refuses while a change is left, a submodule is initialized or the worktree is locked, and
/// deletes `delete`, a branch git must agree is merged. The result names what moved and what
/// would not, then git's words.
pub(super) async fn remove(
    origin: &Path,
    path: &Path,
    keep: &Path,
    delete: Option<&str>,
) -> Result<String, Error> {
    let listing = ["status", "--porcelain", "-z", "--no-renames", "-unormal"];
    let status = run(git(origin).arg("-C").arg(path).args(listing)).await?;
    let whole = status.code == Some(0) && (status.out.len() as u64) < KEPT;
    let merging = names(&status.out).any(|e| e.get(..2).is_some_and(|xy| xy.contains(&b'U')));
    // `XY name`, one name each, since renames are listed as a deletion and an addition; a path
    // deleted in either column is not in the worktree, unless it is unmerged: then it stays, and
    // nothing is reset under it.
    let held = names(&status.out).filter(|e| whole && e.len() > 3 && !e[..2].contains(&b'D'));
    let (mut moved, mut failed) = (Vec::new(), String::new());
    for name in held.map(|e| Path::new(OsStr::from_bytes(&e[3..]))) {
        let to = keep.join(name);
        let made = match fs::create_dir_all(to.parent().unwrap_or(keep)).await {
            Ok(()) => fs::rename(path.join(name), &to).await,
            err => err,
        };
        match made {
            Ok(()) => moved.push(format!("`{}`", name.display())),
            Err(e) => failed += &format!("Could not move `{}`: {e}.\n", name.display()),
        }
    }
    let mut text = String::new();
    if !moved.is_empty() {
        let (keep, moved) = (keep.display(), moved.join(", "));
        text = format!("Moved into {keep}, at their paths: {moved}.\n");
    }
    if whole && failed.is_empty() && !merging {
        run(git(path).args(["reset", "-q", "--hard"])).await?;
    }
    text += &failed;
    let removed = run(git(origin).args(["worktree", "remove"]).arg(path)).await?;
    if removed.code != Some(0) {
        let path = path.display();
        text += &format!("The decision is recorded; git would not remove the worktree at {path}: ");
        return Ok(text + &removed.text);
    }
    text += &removed.text;
    if let Some(branch) = delete {
        text += &run(git(origin).args(["branch", "-d", branch])).await?.text;
    }
    Ok(text)
}
