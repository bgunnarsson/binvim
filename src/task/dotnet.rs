//! Dotnet task discovery — surfaces the well-known verbs against the
//! enclosing `.sln` / `.csproj` / `.fsproj`. There's no "scripts" file
//! to parse; the verbs are part of the dotnet CLI itself, and a user
//! editing a C# / F# project will reasonably expect `:task` to list
//! them. Less universal than the other adapters (the verbs don't take
//! arguments here, so the picker is fixed), but it's the bridge to a
//! consistent "F5 to build" feel for .NET projects.

use std::path::Path;

use super::types::{Task, TaskSource};

pub const ROOT_MARKERS: &[&str] = &["*.sln", "*.csproj", "*.fsproj"];

/// dotnet CLI verbs we surface. `build` / `run` / `test` are the
/// daily-driver three; `restore` / `clean` / `publish` are the rest of
/// the standard kit.
const VERBS: &[(&str, &str)] = &[
    ("build", "dotnet build"),
    ("run", "dotnet run"),
    ("test", "dotnet test"),
    ("restore", "dotnet restore"),
    ("clean", "dotnet clean"),
    ("publish", "dotnet publish"),
];

pub fn discover(root: &Path) -> Vec<Task> {
    VERBS
        .iter()
        .map(|(verb, desc)| Task {
            label: (*verb).to_string(),
            source: TaskSource::Dotnet,
            cwd: root.to_path_buf(),
            program: "dotnet".to_string(),
            args: vec![(*verb).to_string()],
            description: Some((*desc).to_string()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_six_well_known_verbs() {
        let t = discover(Path::new("."));
        assert_eq!(t.len(), 6);
        assert_eq!(t[0].label, "build");
        assert!(t.iter().any(|x| x.label == "test"));
        assert!(t.iter().any(|x| x.label == "publish"));
    }
}
