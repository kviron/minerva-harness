use openidea_plugin::host::{self, Project, storage};
use openidea_plugin::views::{Body, View, ViewEvent, Viewport, Views};
use openidea_plugin::{Error, FileDiff, Invocation, Plugin, register_plugin};
use serde_json::{Value, json};

#[derive(Default)]
struct Subversion;
impl Plugin for Subversion {
    fn run_command(&mut self, id: &str, _: &Invocation, _: Option<&str>) -> Result<Option<String>, Error> {
        if id != "open" {
            return Err(Error::NotFound(id.into()));
        }
        host::reveal_view("svn")?;
        Ok(None)
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

fn svn(project: &Project, command: &str, options: &[&str], targets: &[String]) -> Result<String, Error> {
    let mut args = vec![command.into(), "--non-interactive".into()];
    args.extend(options.iter().map(|arg| (*arg).into()));
    args.push("--".into());
    args.extend(targets.iter().enumerate().map(|(index, path)| {
        if matches!(command, "switch" | "merge") && index == 1 { path.clone() } else { format!("{path}@") }
    }));
    let out = project.run_process("svn", &args, None)?;
    if out.status != Some(0) {
        return Err(invalid(out.stderr));
    }
    Ok(out.stdout)
}

fn relative(path: &str) -> Result<String, Error> {
    let path = path.trim().replace('\\', "/");
    if path.is_empty()
        || path.starts_with('/')
        || path.contains([':', '\0'])
        || path.split('/').any(|part| part == "..")
    {
        return Err(invalid("Enter a folder relative to the project."));
    }
    let path = path.split('/').filter(|part| !part.is_empty() && *part != ".").collect::<Vec<_>>().join("/");
    Ok(if path.is_empty() { ".".into() } else { path })
}

fn changes(project: &Project, root: &str) -> Result<Vec<Value>, Error> {
    let xml = svn(project, "status", &["--xml", "--ignore-externals"], &[root.into()])?;
    let document = roxmltree::Document::parse(&xml).map_err(|e| invalid(e.to_string()))?;
    Ok(document
        .descendants()
        .filter(|node| node.has_tag_name("entry"))
        .filter_map(|entry| {
            let status = entry.children().find(|node| node.has_tag_name("wc-status"))?;
            let path = entry.attribute("path")?;
            let item = status.attribute("item")?;
            let conflict = item == "conflicted"
                || status.attribute("tree-conflicted") == Some("true")
                || status.attribute("props") == Some("conflicted");
            Some(json!({"path": path, "status": item, "conflict": conflict}))
        })
        .collect())
}

fn field<'a>(request: &'a Value, key: &str) -> Result<&'a str, Error> {
    request[key].as_str().ok_or_else(|| invalid(format!("Missing {key}")))
}

fn execute(project: &Project, request: Value) -> Result<Value, Error> {
    let key = format!("roots:{}", project.root()?);
    let mut roots: Vec<String> =
        storage::get(&key)?.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    let op = field(&request, "op")?;
    if op == "roots" {
        return Ok(json!({"roots": roots}));
    }
    let root = relative(field(&request, "root")?)?;
    if op == "connect" {
        let wc_root = svn(project, "info", &["--show-item", "wc-root"], &[root.clone()])?;
        let project_root = project.root()?.replace('\\', "/");
        let expected = if root == "." {
            project_root.trim_end_matches('/').to_owned()
        } else {
            format!("{}/{root}", project_root.trim_end_matches('/'))
        };
        let actual = wc_root.trim().replace('\\', "/");
        let matches =
            actual == expected || (expected.as_bytes().get(1) == Some(&b':') && actual.eq_ignore_ascii_case(&expected));
        if !matches {
            return Err(invalid("Select the root folder of a Subversion working copy inside this project."));
        }
        if !roots.contains(&root) {
            roots.push(root.clone());
            storage::set(&key, &json!(roots).to_string())?;
        }
        return Ok(json!({"roots": roots, "current": root}));
    }
    if !roots.contains(&root) {
        return Err(invalid("Select a connected working copy."));
    }
    let target = [root.clone()];
    match op {
        "disconnect" => {
            roots.retain(|saved| saved != &root);
            storage::set(&key, &json!(roots).to_string())?;
            return Ok(json!({"roots": roots}));
        }
        "history" => {
            let xml = svn(project, "log", &["--xml", "-l", "100"], &target)?;
            let document = roxmltree::Document::parse(&xml).map_err(|e| invalid(e.to_string()))?;
            let rows: Vec<_> = document.descendants().filter(|n| n.has_tag_name("logentry")).map(|row| {
                let value = |name| row.children().find(|n| n.has_tag_name(name)).and_then(|n| n.text()).unwrap_or("");
                json!({"revision":row.attribute("revision"), "author":value("author"), "date":value("date"), "message":value("msg")})
            }).collect();
            return Ok(json!({"history": rows}));
        }
        "update" => {
            svn(project, "update", &["--ignore-externals", "--accept", "postpone"], &target)?;
        }
        "switch" | "merge" => {
            let url = field(&request, "url")?.trim();
            if !(url.starts_with("^/")
                || ["https://", "http://", "svn://", "svn+ssh://", "file://"]
                    .iter()
                    .any(|prefix| url.starts_with(prefix)))
            {
                return Err(invalid("Enter an SVN repository URL or a ^/ path."));
            }
            let options: &[&str] = if op == "switch" {
                &["--ignore-externals", "--accept", "postpone"]
            } else {
                &["--accept", "postpone"]
            };
            svn(project, op, options, &[url.into(), root.clone()])?;
        }
        "status" => {}
        "diff" | "add" | "commit" | "revert" => {
            let rows = changes(project, &root)?;
            let selected = request["paths"].as_array().ok_or_else(|| invalid("Select files."))?;
            if selected.is_empty() || (op == "diff" && selected.len() != 1) {
                return Err(invalid("Select files."));
            }
            let mut paths = Vec::new();
            let mut statuses = Vec::new();
            for path in selected {
                let path = path.as_str().ok_or_else(|| invalid("Invalid file selection."))?;
                let normalized = relative(path)?;
                if !rows.iter().any(|row| row["path"] == path)
                    || (root != "." && normalized != root && !normalized.starts_with(&format!("{root}/")))
                {
                    return Err(invalid("The selection changed. Refresh the working copy."));
                }
                let row = rows.iter().find(|row| row["path"] == path).unwrap();
                statuses.push(row["status"].as_str().unwrap_or(""));
                paths.push(normalized);
            }
            match op {
                "diff" => {
                    let path = &paths[0];
                    let left = match statuses[0] {
                        "added" | "unversioned" => None,
                        _ => Some(svn(project, "cat", &["-r", "BASE"], &paths)?),
                    };
                    let right = match statuses[0] {
                        "deleted" | "missing" => None,
                        _ => Some(project.read_file(path)?),
                    };
                    host::open_file_diff(
                        &FileDiff {
                            left,
                            right,
                            left_label: Some("BASE".into()),
                            right_label: Some("Working copy".into()),
                            title: Some(path.clone()),
                        },
                        path.rsplit('/').next(),
                    )?;
                    return Ok(json!({}));
                }
                "commit" => {
                    let message = field(&request, "message")?.trim();
                    if message.is_empty()
                        || statuses.iter().any(|s| matches!(*s, "unversioned" | "ignored" | "external"))
                    {
                        return Err(invalid("Enter a commit message and select versioned changes."));
                    }
                    svn(project, "commit", &["--depth", "empty", "-m", message], &paths)?;
                }
                "add" => {
                    svn(project, "add", &["--depth", "empty"], &paths)?;
                }
                "revert" => {
                    svn(project, "revert", &["--depth", "empty"], &paths)?;
                }
                _ => unreachable!(),
            }
        }
        _ => return Err(invalid("Unknown Subversion action.")),
    }
    let url = svn(project, "info", &["--show-item", "url"], &target)?;
    Ok(json!({"changes": changes(project, &root)?, "url": url.trim()}))
}

impl Views for Subversion {
    fn render(&mut self, _: &str, _: Option<Viewport>, _: &Invocation) -> Result<View, Error> {
        Ok(View::new(Body::Html(include_str!("page.html").into())))
    }

    fn handle(&mut self, view: &str, event: ViewEvent, ctx: &Invocation) -> Result<(), Error> {
        if let ViewEvent::Message(message) = event {
            let result = ctx.project().ok_or_else(|| invalid("Open a project first.")).and_then(|project| {
                let request = serde_json::from_str(&message).map_err(|e| invalid(format!("Invalid request: {e}")))?;
                execute(&project, request)
            });
            let reply = match result {
                Ok(value) => value,
                Err(error) => json!({"error":error.to_string()}),
            };
            host::post_view_message(view, &reply.to_string())?;
        }
        Ok(())
    }
}
register_plugin!(Subversion, views);
