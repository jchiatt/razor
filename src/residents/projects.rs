//! A resident's projects: each folder in its `projects` folder, whose name never changes, shown
//! by the name in its `.razor/name`, so a folder the resident makes there is a project too.

use super::{Error, Residents, TEXT, other};
use crate::protocol::{ResidentProject, SessionId, millis};
use std::{cmp::Reverse, io::ErrorKind, path::Path, path::PathBuf};
use tokio::{fs, io::AsyncReadExt, sync::Mutex};

/// Held while a project is made or renamed.
static WRITING: Mutex<()> = Mutex::const_new(());

impl Residents {
    /// Each project but a hidden folder or a link, newest chat activity first; a project with
    /// no chat sorts by its folder's last change.
    pub(crate) async fn projects(
        &self,
        resident: &SessionId,
    ) -> Result<Vec<ResidentProject>, Error> {
        let dir = self.resident(resident).await?.cwd.join("projects");
        let home = self.inner.controller.home();
        let roots = crate::record::list(home, None, Some(resident), false, usize::MAX).await;
        let roots = roots.map_err(other("listing the resident's chats"))?;
        let threads: Vec<_> = (roots.into_iter())
            .filter_map(|(_, _, t, _, at)| Some((t?.resident_project?, at)))
            .collect();
        let mut projects = Vec::new();
        let Ok(mut folders) = fs::read_dir(&dir).await else {
            return Ok(projects);
        };
        while let Ok(Some(folder)) = folders.next_entry().await {
            let id = folder.file_name().to_string_lossy().into_owned();
            let meta = folder.metadata().await;
            if id.starts_with('.') || !meta.as_ref().is_ok_and(|m| m.is_dir()) {
                continue;
            }
            let chats: Vec<_> = threads.iter().filter(|t| t.0 == id).map(|t| t.1).collect();
            let changed = meta.ok().and_then(|m| m.modified().ok());
            let at = chats.iter().max().copied().or(changed).map_or(0, millis);
            let name = small(&folder.path().join(".razor/name")).await;
            projects.push(ResidentProject {
                name: name.unwrap_or_else(|| id.clone()),
                id,
                at,
                chats: chats.len(),
            });
        }
        projects.sort_by_key(|p| Reverse(p.at));
        Ok(projects)
    }

    /// Project `id`'s folder: a folder, not a link, directly in the resident's `projects`.
    pub(crate) async fn project(&self, resident: &SessionId, id: &str) -> Result<PathBuf, Error> {
        let dir = self.resident(resident).await?.cwd.join("projects").join(id);
        let listed = !id.is_empty() && !id.starts_with('.') && !id.contains(['/', '\\']);
        match fs::symlink_metadata(&dir).await {
            Ok(meta) if listed && meta.is_dir() => Ok(dir),
            _ => Err(Error::NoProject(TEXT.no_project.replace("{id}", id))),
        }
    }

    /// Makes a project named `name` in a new folder of that name, or with `id` renames that
    /// project and leaves its folder; making one again with the same `request` answers the
    /// project that request made, unchanged.
    pub(crate) async fn name_project(
        &self,
        resident: &SessionId,
        id: Option<&str>,
        name: &str,
        request: Option<&str>,
    ) -> Result<ResidentProject, Error> {
        let name = name.trim();
        let fits = name.chars().count() <= 80 && !name.contains(char::is_control);
        if !fits || !name.contains(char::is_alphanumeric) {
            return Err(Error::Refused(TEXT.project_name.clone()));
        }
        // One write at a time, so a request sent twice at once finds the first one's receipt.
        let _one = WRITING.lock().await;
        let (id, dir, request) = match id {
            Some(id) => (id.to_owned(), Some(self.project(resident, id).await?), None),
            None => {
                let projects = self.resident(resident).await?.cwd.join("projects");
                let made = fs::create_dir_all(&projects).await;
                made.map_err(other("making the projects folder"))?;
                // At most 48 characters, so at most 192 bytes, as any file system allows.
                let slug = name.replace(['/', '\\'], "-");
                let slug: String = slug.trim_start_matches('.').chars().take(48).collect();
                let (mut id, mut n) = (slug.clone(), 1);
                loop {
                    let dir = projects.join(&id);
                    match fs::create_dir(&dir).await {
                        Ok(()) => break (id, Some(dir), request),
                        Err(e) if e.kind() != ErrorKind::AlreadyExists => {
                            return Err(other("making a project")(e));
                        }
                        // A project's own receipt, never one through a link, answers it unchanged.
                        Err(_) if self.project(resident, &id).await.is_ok() => {
                            let receipt = small(&dir.join(".razor/request")).await;
                            if request.is_some() && receipt.as_deref() == request {
                                break (id, None, None);
                            }
                        }
                        Err(_) => {}
                    }
                    n += 1;
                    id = format!("{slug}-{n}");
                }
            }
        };
        if let Some(dir) = dir {
            let razor = dir.join(".razor");
            let written = async {
                fs::create_dir_all(&razor).await?;
                if let Some(request) = request {
                    fs::write(razor.join("request"), request).await?;
                }
                fs::write(razor.join("name"), name).await
            };
            written.await.map_err(other("naming a project"))?;
        }
        let projects = self.projects(resident).await?;
        let found = projects.into_iter().find(|p| p.id == id);
        found.ok_or_else(|| Error::NoProject(TEXT.no_project.replace("{id}", &id)))
    }
}

/// The first line of a file the resident may edit, trimmed, from its first KiB; none when the
/// file is absent or that line is empty.
async fn small(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    let file = fs::File::open(path).await.ok()?;
    file.take(1024).read_to_end(&mut bytes).await.ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}
