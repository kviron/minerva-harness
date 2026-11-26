//! Restricted model-facing tool profile; host enablement requires separate live evidence.
use super::cli::Start;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub(super) enum Profile {
    ReadOnly,
    WorkspaceWrite,
}

impl Profile {
    fn mode(self) -> &'static str {
        match self {
            Self::ReadOnly => "dontAsk",
            Self::WorkspaceWrite => "manual",
        }
    }

    fn tools(self) -> &'static [&'static str] {
        match self {
            Self::ReadOnly => &["Read", "Glob", "Grep"],
            Self::WorkspaceWrite => &["Read", "Glob", "Grep", "Write", "Edit"],
        }
    }

    pub(super) fn workspace_input(self, cwd: &Path, name: &str, input: &Value) -> Option<Value> {
        if !self.tools().contains(&name) {
            return None;
        }
        let mut updated = input.as_object()?.clone();
        let file = matches!(name, "Read" | "Write" | "Edit");
        let key = if file { "file_path" } else { "path" };
        let requested = match updated.get(key) {
            Some(value) => PathBuf::from(value.as_str().filter(|path| !path.is_empty())?),
            None if !file => PathBuf::from("."),
            None => return None,
        };
        let root = dunce::canonicalize(cwd).ok()?;
        let absolute = root.join(requested);
        let target = match dunce::canonicalize(&absolute) {
            Ok(target) => target,
            Err(error) if name == "Write" && error.kind() == std::io::ErrorKind::NotFound => {
                // Do not reinterpret a broken symlink as a new file.
                if std::fs::symlink_metadata(&absolute).err()?.kind() != std::io::ErrorKind::NotFound {
                    return None;
                }
                let parent = dunce::canonicalize(absolute.parent()?).ok()?;
                let filename = absolute.file_name()?;
                #[cfg(windows)]
                if filename.to_str()?.contains(':') {
                    return None;
                }
                if !parent.is_dir() {
                    return None;
                }
                parent.join(filename)
            }
            Err(_) => return None,
        };
        if !target.starts_with(&root) || (matches!(name, "Write" | "Edit") && target == root) {
            return None;
        }
        if matches!(name, "Write" | "Edit") && target.exists() && !target.is_file() {
            return None;
        }
        updated.insert(key.into(), Value::String(target.to_str()?.into()));
        Some(Value::Object(updated))
    }

    pub(super) fn args(self, start: &Start, mcp_config: &Path) -> Vec<String> {
        let mut args = start.args();
        args.retain(|arg| arg != "--allow-dangerously-skip-permissions");
        let prompts = match self {
            Self::ReadOnly => "none",
            Self::WorkspaceWrite => "host",
        };
        args.extend(
            [
                "--safe-mode",
                "--restricted",
                "--permission-mode",
                self.mode(),
                "--permission-prompts",
                prompts,
                "--tools",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        args.push(self.tools().join(","));
        if matches!(self, Self::WorkspaceWrite) {
            args.extend(["--settings".into(), json!({"permissions":{"ask":["Write","Edit"]}}).to_string()]);
        }

        args.extend(["--strict-mcp-config".into(), "--mcp-config".into()]);
        args.push(mcp_config.to_string_lossy().into_owned());
        args
    }

    pub(super) fn verify_permission_mode(self, init: &Value) -> Result<(), String> {
        let reported = init["current_permission_mode"].as_str();
        // Claude's CLI names this mode manual and reports its protocol name default.
        if reported == Some(self.mode()) || (matches!(self, Self::WorkspaceWrite) && reported == Some("default")) {
            Ok(())
        } else {
            Err("Claude Code did not confirm the delegated permission mode".into())
        }
    }

    pub(super) fn verify_inventory(self, event: &Value, expected_session: &str) -> Result<(), String> {
        if event["session_id"].as_str() != Some(expected_session) {
            return Err("Claude Code did not confirm the original delegated session".into());
        }
        let tools_match = event["tools"].as_array().is_some_and(|reported| {
            reported.len() == self.tools().len()
                && self.tools().iter().all(|name| reported.iter().any(|value| value.as_str() == Some(name)))
        });
        let no_mcp = event["mcp_servers"].as_array().is_some_and(Vec::is_empty);
        if tools_match && no_mcp {
            Ok(())
        } else {
            Err("Claude Code reported an unexpected delegated tool or MCP inventory".into())
        }
    }
}

pub(super) fn permission(cwd: &Path, name: &str, input: &Value, tool_use: &str) -> Value {
    let mut deny = "This delegated research session permits Read, Glob and Grep only";
    if Profile::ReadOnly.tools().contains(&name) {
        if let Some(updated) = Profile::ReadOnly.workspace_input(cwd, name, input) {
            return json!({"behavior":"allow","updatedInput":updated,"toolUseID":tool_use});
        }
        deny = "This delegated research session requires an existing path inside the workspace";
    }
    json!({"behavior":"deny","message":deny,"toolUseID":tool_use})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writable_profile_requires_host_approval_and_exact_file_tools() {
        let profile = Profile::WorkspaceWrite;
        let args = profile.args(&Start::New { id: "session".into() }, Path::new("mcp.json"));
        for expected in [
            ["--permission-mode", "manual"],
            ["--permission-prompts", "host"],
            ["--tools", "Read,Glob,Grep,Write,Edit"],
        ] {
            assert!(args.windows(2).any(|pair| pair == expected));
        }
        assert!(profile.verify_permission_mode(&json!({"current_permission_mode":"manual"})).is_ok());
        assert!(profile.verify_permission_mode(&json!({"current_permission_mode":"default"})).is_ok());
        assert!(profile.verify_permission_mode(&json!({"current_permission_mode":"acceptEdits"})).is_err());
        let init = json!({"session_id":"session","tools":["Read","Glob","Grep","Write","Edit"],"mcp_servers":[]});
        assert!(profile.verify_inventory(&init, "session").is_ok());
        assert!(Profile::ReadOnly.verify_inventory(&init, "session").is_err());
        let mut wider = init;
        wider["tools"][4] = json!("Bash");
        assert!(profile.verify_inventory(&wider, "session").is_err());
    }

    #[test]
    fn write_paths_are_scoped_and_new_files_require_an_existing_parent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("existing.txt"), "inside").unwrap();
        std::fs::write(temp.path().join("outside.txt"), "outside").unwrap();
        let profile = Profile::WorkspaceWrite;
        let verify = |tool, path| profile.workspace_input(&root, tool, &json!({"file_path": path}));
        assert!(verify("Write", "new.txt").is_some());
        assert!(verify("Edit", "existing.txt").is_some());
        assert!(verify("Edit", "new.txt").is_none());
        for invalid in ["../outside.txt", "../new.txt", "missing/new.txt", ".", ""] {
            assert!(verify("Write", invalid).is_none(), "{invalid}");
        }
        assert!(verify("Bash", "existing.txt").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn write_paths_reject_junctions_outside_the_project() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(root.join("linked"))
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let input = json!({"file_path":"linked/new.txt"});
        assert!(Profile::WorkspaceWrite.workspace_input(&root, "Write", &input).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn write_paths_reject_dangling_and_outside_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(temp.path(), root.join("outside")).unwrap();
        std::os::unix::fs::symlink(temp.path().join("missing"), root.join("dangling")).unwrap();
        let profile = Profile::WorkspaceWrite;
        for invalid in ["outside/new.txt", "dangling"] {
            assert!(profile.workspace_input(&root, "Write", &json!({"file_path":invalid})).is_none());
        }
    }

    #[test]
    fn fixed_launch_drops_bypass_and_restricts_tools() {
        let args = Profile::ReadOnly.args(&Start::New { id: "test".into() }, std::path::Path::new("mcp.json"));
        assert!(!args.iter().any(|arg| arg.contains("skip-permissions")));
        assert!(args.iter().any(|arg| arg == "--restricted"));
        for expected in
            [["--permission-mode", "dontAsk"], ["--permission-prompts", "none"], ["--tools", "Read,Glob,Grep"]]
        {
            assert!(args.windows(2).any(|pair| pair == expected));
        }
        for name in ["Write", "Edit", "Bash", "Agent", "Skill", "mcp__server__read"] {
            assert_eq!(permission(Path::new("."), name, &json!({}), "call")["behavior"], "deny");
        }
    }

    #[test]
    fn delegated_approval_resolves_paths_against_the_workspace() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("inside"), "").unwrap();
        let outside = parent.path().join("outside");
        std::fs::write(&outside, "").unwrap();
        let approve = |name, input| permission(&root, name, &input, "call");
        let read = approve("Read", json!({"file_path":"inside","limit":1}));
        assert_eq!(read["behavior"], "allow");
        assert_eq!(
            read["updatedInput"]["file_path"],
            dunce::canonicalize(root.join("inside")).unwrap().to_str().unwrap()
        );
        assert_eq!(read["updatedInput"]["limit"], 1);
        assert_eq!(approve("Glob", json!({"pattern":"*.rs"}))["behavior"], "allow");
        for input in
            [json!({}), json!({"file_path":""}), json!({"file_path":"../outside"}), json!({"file_path":outside})]
        {
            assert_eq!(approve("Read", input)["behavior"], "deny");
        }
        assert_eq!(approve("Grep", json!({"path":null}))["behavior"], "deny");
    }

    #[test]
    fn permission_mode_must_be_confirmed_by_the_process() {
        assert!(Profile::ReadOnly.verify_permission_mode(&json!({"current_permission_mode": "dontAsk"})).is_ok());
        assert!(
            Profile::ReadOnly.verify_permission_mode(&json!({"current_permission_mode": "bypassPermissions"})).is_err()
        );
        assert!(Profile::ReadOnly.verify_permission_mode(&json!({})).is_err());
    }

    #[test]
    fn delegated_inventory_must_match_the_allowlist_and_have_no_mcp() {
        let verify = |tools: Value, mcp: Value| {
            Profile::ReadOnly
                .verify_inventory(&json!({"session_id":"expected","tools":tools,"mcp_servers":mcp}), "expected")
        };
        assert!(verify(json!(["Glob", "Grep", "Read"]), json!([])).is_ok());
        assert!(verify(json!(["Glob", "Grep", "Read", "Write"]), json!([])).is_err());
        assert!(verify(json!(["Glob", "Grep", "Read"]), json!([{"name":"external"}])).is_err());
        assert!(verify(json!(["Glob", "Grep", "Read"]), Value::Null).is_err());
        assert!(
            Profile::ReadOnly
                .verify_inventory(
                    &json!({"session_id":"other","tools":["Glob","Grep","Read"],"mcp_servers":[]}),
                    "expected"
                )
                .is_err()
        );
    }
}
