use serde_json::{Value, json};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "AGENTS.md";

pub struct Instructions {
    path: PathBuf,
    text: String,
}

/// Reads every `AGENTS.md` from the current directory up to the filesystem root,
/// outermost first, so the nearest file comes last and takes precedence.
/// Missing or empty files are skipped.
pub fn load() -> Result<Vec<Instructions>, String> {
    let current = std::env::current_dir()
        .map_err(|error| format!("could not find the current directory: {error}"))?;

    let mut found = Vec::new();
    for path in candidate_paths(&current) {
        if let Some(text) = read(&path)? {
            found.push(Instructions { path, text });
        }
    }
    Ok(found)
}

fn candidate_paths(directory: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = directory
        .ancestors()
        .map(|ancestor| ancestor.join(FILE_NAME))
        .collect();
    paths.reverse();
    paths
}

fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(None),
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

/// The identity line stays alone in the first block; each file gets its own block after it.
pub fn system_prompt(identity: &str, instructions: &[Instructions]) -> Value {
    let mut blocks = vec![json!({"type": "text", "text": identity})];
    for file in instructions {
        blocks.push(json!({
            "type": "text",
            "text": format!(
                "Instructions from {}:\n\n{}",
                file.path.display(),
                file.text.trim_end()
            ),
        }));
    }
    Value::Array(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_only_the_identity_without_instructions() {
        assert_eq!(
            system_prompt("identity", &[]),
            json!([{"type": "text", "text": "identity"}])
        );
    }

    #[test]
    fn adds_each_instructions_file_after_the_identity_in_order() {
        let files = [
            Instructions {
                path: "/home/a/AGENTS.md".into(),
                text: "outer\n".into(),
            },
            Instructions {
                path: "/project/AGENTS.md".into(),
                text: "project".into(),
            },
        ];

        assert_eq!(
            system_prompt("identity", &files),
            json!([
                {"type": "text", "text": "identity"},
                {"type": "text", "text": "Instructions from /home/a/AGENTS.md:\n\nouter"},
                {"type": "text", "text": "Instructions from /project/AGENTS.md:\n\nproject"},
            ])
        );
    }

    #[test]
    fn looks_from_the_root_down_to_the_current_directory() {
        assert_eq!(
            candidate_paths(Path::new("/home/a/project")),
            vec![
                PathBuf::from("/AGENTS.md"),
                PathBuf::from("/home/AGENTS.md"),
                PathBuf::from("/home/a/AGENTS.md"),
                PathBuf::from("/home/a/project/AGENTS.md"),
            ]
        );
    }

    #[test]
    fn skips_a_missing_or_empty_file() {
        let directory =
            std::env::temp_dir().join(format!("pi-rust-instructions-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let empty = directory.join("AGENTS.md");
        std::fs::write(&empty, "  \n").unwrap();

        assert_eq!(read(&directory.join("missing.md")).unwrap(), None);
        assert_eq!(read(&empty).unwrap(), None);

        std::fs::remove_dir_all(&directory).unwrap();
    }
}
