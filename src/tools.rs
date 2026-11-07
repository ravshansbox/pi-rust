use serde_json::{Value, json};
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const OUTPUT_LIMIT: usize = 50_000;
const DEFAULT_TIMEOUT_SECONDS: usize = 120;

pub fn definitions() -> Value {
    json!([
        {
            "name": "read",
            "description": "Read a text file. Use offset and limit to read part of a large file.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the file"},
                    "offset": {"type": "integer", "minimum": 1, "description": "Line number to start from (1-based)"},
                    "limit": {"type": "integer", "minimum": 1, "description": "Maximum number of lines to read"}
                },
                "required": ["path"]
            }
        },
        {
            "name": "write",
            "description": "Write a file, replacing it if it exists. Creates parent directories.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the file"},
                    "content": {"type": "string", "description": "Full content of the file"}
                },
                "required": ["path", "content"]
            }
        },
        {
            "name": "edit",
            "description": "Edit a file by replacing exact text. Each oldText must match exactly once in the original file, and edits must not overlap.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the file"},
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": {"type": "string", "description": "Exact text to replace"},
                                "newText": {"type": "string", "description": "Replacement text"}
                            },
                            "required": ["oldText", "newText"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }
        },
        {
            "name": "bash",
            "description": "Run a shell command with sh in the current directory. Returns stdout and stderr combined, followed by the exit code.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Command to run"},
                    "timeout": {"type": "integer", "minimum": 1, "description": "Timeout in seconds (default 120)"}
                },
                "required": ["command"]
            }
        }
    ])
}

pub fn summary(name: &str, input: &Value) -> String {
    let detail = match name {
        "read" | "write" | "edit" => input.get("path"),
        "bash" => input.get("command"),
        _ => None,
    }
    .and_then(Value::as_str)
    .unwrap_or_default();
    format!("{name} {detail}")
}

pub async fn run(name: &str, input: &Value) -> Result<String, String> {
    let result = match name {
        "read" => read(input),
        "write" => write(input),
        "edit" => edit(input),
        "bash" => bash(input).await,
        _ => Err(format!("unknown tool `{name}`")),
    };
    result.map(truncate).map_err(truncate)
}

fn read(input: &Value) -> Result<String, String> {
    let path = text_field(input, "path")?;
    let offset = number_field(input, "offset")?.unwrap_or(1);
    let limit = number_field(input, "limit")?.unwrap_or(usize::MAX);
    let contents =
        fs::read_to_string(path).map_err(|error| format!("could not read {path}: {error}"))?;
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();

    if offset == 0 || (offset > 1 && offset > lines.len()) {
        return Err(format!(
            "offset {offset} is outside {path}, which has {} lines",
            lines.len()
        ));
    }

    Ok(lines.iter().skip(offset - 1).take(limit).copied().collect())
}

fn write(input: &Value) -> Result<String, String> {
    let path = text_field(input, "path")?;
    let content = text_field(input, "content")?;

    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    fs::write(path, content).map_err(|error| format!("could not write {path}: {error}"))?;

    Ok(format!("wrote {} bytes to {path}", content.len()))
}

fn edit(input: &Value) -> Result<String, String> {
    let path = text_field(input, "path")?;
    let edits = input
        .get("edits")
        .and_then(Value::as_array)
        .filter(|edits| !edits.is_empty())
        .ok_or("`edits` must be a non-empty array")?;
    let original =
        fs::read_to_string(path).map_err(|error| format!("could not read {path}: {error}"))?;

    let mut replacements = Vec::new();
    for (index, edit) in edits.iter().enumerate() {
        let number = index + 1;
        let old_text =
            text_field(edit, "oldText").map_err(|error| format!("edit {number}: {error}"))?;
        let new_text =
            text_field(edit, "newText").map_err(|error| format!("edit {number}: {error}"))?;
        if old_text.is_empty() {
            return Err(format!("edit {number}: `oldText` must not be empty"));
        }

        let mut matches = original.match_indices(old_text);
        let Some((start, _)) = matches.next() else {
            return Err(format!("edit {number}: `oldText` was not found in {path}"));
        };
        if matches.next().is_some() {
            return Err(format!(
                "edit {number}: `oldText` matches more than once in {path}; include more context"
            ));
        }
        replacements.push((start, start + old_text.len(), new_text));
    }

    replacements.sort_by_key(|&(start, _, _)| start);
    if replacements.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("edits overlap; merge them into one edit".to_string());
    }

    let mut updated = String::new();
    let mut position = 0;
    for &(start, end, new_text) in &replacements {
        updated.push_str(&original[position..start]);
        updated.push_str(new_text);
        position = end;
    }
    updated.push_str(&original[position..]);

    fs::write(path, updated).map_err(|error| format!("could not write {path}: {error}"))?;

    Ok(format!("applied {} edits to {path}", replacements.len()))
}

async fn bash(input: &Value) -> Result<String, String> {
    let command = text_field(input, "command")?;
    let seconds = number_field(input, "timeout")?.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    let (mut reader, writer) =
        io::pipe().map_err(|error| format!("could not create a pipe: {error}"))?;
    let error_writer = writer
        .try_clone()
        .map_err(|error| format!("could not create a pipe: {error}"))?;

    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(error_writer)
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not run `sh`: {error}"))?;

    let process_group = child.id().map(|pid| format!("-{pid}"));
    let reading = tokio::task::spawn_blocking(move || {
        let mut output = Vec::new();
        reader.read_to_end(&mut output).map(|_| output)
    });
    let finished = tokio::time::timeout(Duration::from_secs(seconds as u64), async {
        (child.wait().await, reading.await)
    })
    .await;

    let Ok((status, output)) = finished else {
        if let Some(process_group) = process_group {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &process_group])
                .status();
        }
        let _ = child.kill().await;
        return Err(format!("command timed out after {seconds} seconds"));
    };
    let status = status.map_err(|error| format!("could not wait for the command: {error}"))?;
    let output = output
        .map_err(|error| format!("could not read the command output: {error}"))?
        .map_err(|error| format!("could not read the command output: {error}"))?;

    let exit = match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "killed by a signal".to_string(),
    };
    Ok(format!("{}{exit}", String::from_utf8_lossy(&output)))
}

fn text_field<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` must be a string"))
}

fn number_field(input: &Value, key: &str) -> Result<Option<usize>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|number| Some(number as usize))
            .ok_or_else(|| format!("`{key}` must be a positive integer")),
    }
}

fn truncate(text: String) -> String {
    if text.len() <= OUTPUT_LIMIT {
        return text;
    }

    let mut end = OUTPUT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[truncated: showing {end} of {} bytes]",
        &text[..end],
        text.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let directory = env::temp_dir().join(format!("pi-rust-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn path_text(path: &Path) -> &str {
        path.to_str().unwrap()
    }

    #[tokio::test]
    async fn reads_a_whole_file() {
        let file = scratch("read-whole").join("notes.txt");
        fs::write(&file, "one\ntwo\n").unwrap();

        let output = run("read", &json!({"path": path_text(&file)})).await;

        assert_eq!(output.unwrap(), "one\ntwo\n");
    }

    #[tokio::test]
    async fn reads_a_range_of_lines() {
        let file = scratch("read-range").join("notes.txt");
        fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();

        let output = run(
            "read",
            &json!({"path": path_text(&file), "offset": 2, "limit": 2}),
        )
        .await;

        assert_eq!(output.unwrap(), "two\nthree\n");
    }

    #[tokio::test]
    async fn rejects_an_offset_past_the_end() {
        let file = scratch("read-offset").join("notes.txt");
        fs::write(&file, "one\n").unwrap();

        let error = run("read", &json!({"path": path_text(&file), "offset": 5}))
            .await
            .unwrap_err();

        assert!(error.contains("has 1 lines"), "{error}");
    }

    #[tokio::test]
    async fn reports_a_missing_file() {
        let error = run("read", &json!({"path": "/no/such/file"}))
            .await
            .unwrap_err();

        assert!(error.contains("could not read /no/such/file"), "{error}");
    }

    #[tokio::test]
    async fn truncates_large_output() {
        let file = scratch("read-large").join("large.txt");
        fs::write(&file, "é".repeat(OUTPUT_LIMIT)).unwrap();

        let output = run("read", &json!({"path": path_text(&file)}))
            .await
            .unwrap();

        assert!(output.len() < OUTPUT_LIMIT + 100);
        assert!(
            output.ends_with(&format!(
                "[truncated: showing {OUTPUT_LIMIT} of {} bytes]",
                OUTPUT_LIMIT * 2
            )),
            "{}",
            &output[OUTPUT_LIMIT - 20..]
        );
    }

    #[tokio::test]
    async fn writes_a_file_and_its_parent_directories() {
        let file = scratch("write").join("nested/notes.txt");

        let output = run(
            "write",
            &json!({"path": path_text(&file), "content": "héllo\n"}),
        )
        .await;

        assert_eq!(
            output.unwrap(),
            format!("wrote 7 bytes to {}", path_text(&file))
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "héllo\n");
    }

    #[tokio::test]
    async fn replaces_an_existing_file() {
        let file = scratch("write-replace").join("notes.txt");
        fs::write(&file, "old content").unwrap();

        run(
            "write",
            &json!({"path": path_text(&file), "content": "new"}),
        )
        .await
        .unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
    }

    #[tokio::test]
    async fn requires_content_to_write() {
        let file = scratch("write-missing").join("notes.txt");

        let error = run("write", &json!({"path": path_text(&file)}))
            .await
            .unwrap_err();

        assert_eq!(error, "`content` must be a string");
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn applies_several_edits() {
        let file = scratch("edit").join("notes.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();

        let output = run(
            "edit",
            &json!({"path": path_text(&file), "edits": [
                {"oldText": "three", "newText": "3"},
                {"oldText": "one", "newText": "1"},
            ]}),
        )
        .await;

        assert_eq!(
            output.unwrap(),
            format!("applied 2 edits to {}", path_text(&file))
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "1\ntwo\n3\n");
    }

    #[tokio::test]
    async fn rejects_text_that_is_not_found() {
        let file = scratch("edit-missing").join("notes.txt");
        fs::write(&file, "one\n").unwrap();

        let error = run(
            "edit",
            &json!({"path": path_text(&file), "edits": [{"oldText": "two", "newText": "2"}]}),
        )
        .await
        .unwrap_err();

        assert!(error.contains("edit 1: `oldText` was not found"), "{error}");
    }

    #[tokio::test]
    async fn rejects_text_that_matches_more_than_once_and_leaves_the_file_alone() {
        let file = scratch("edit-ambiguous").join("notes.txt");
        fs::write(&file, "one\none\n").unwrap();

        let error = run(
            "edit",
            &json!({"path": path_text(&file), "edits": [
                {"oldText": "\n", "newText": "!"},
            ]}),
        )
        .await
        .unwrap_err();

        assert!(error.contains("matches more than once"), "{error}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "one\none\n");
    }

    #[tokio::test]
    async fn rejects_overlapping_edits() {
        let file = scratch("edit-overlap").join("notes.txt");
        fs::write(&file, "abcdef").unwrap();

        let error = run(
            "edit",
            &json!({"path": path_text(&file), "edits": [
                {"oldText": "abcd", "newText": "x"},
                {"oldText": "cdef", "newText": "y"},
            ]}),
        )
        .await
        .unwrap_err();

        assert!(error.contains("edits overlap"), "{error}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "abcdef");
    }

    #[tokio::test]
    async fn runs_a_command_with_combined_output_and_exit_code() {
        let output = run(
            "bash",
            &json!({"command": "echo out; echo err >&2; echo again; exit 3"}),
        )
        .await;

        assert_eq!(output.unwrap(), "out\nerr\nagain\nexit code 3");
    }

    #[tokio::test]
    async fn runs_a_command_in_the_current_directory() {
        let output = run("bash", &json!({"command": "pwd"})).await.unwrap();

        let directory = env::current_dir().unwrap();
        assert_eq!(output, format!("{}\nexit code 0", directory.display()));
    }

    #[tokio::test]
    async fn stops_a_command_that_runs_too_long() {
        let error = run("bash", &json!({"command": "sleep 5", "timeout": 1}))
            .await
            .unwrap_err();

        assert_eq!(error, "command timed out after 1 seconds");
    }

    #[tokio::test]
    async fn stops_the_processes_a_command_started_when_it_runs_too_long() {
        let pid_file = scratch("bash-group").join("pid");
        let command = format!("sleep 30 & echo $! > {}; wait", path_text(&pid_file));

        run("bash", &json!({"command": command, "timeout": 1}))
            .await
            .unwrap_err();

        let pid = fs::read_to_string(&pid_file).unwrap();
        let running = || {
            std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .status()
                .unwrap()
                .success()
        };
        for _ in 0..20 {
            if !running() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("process {} is still running", pid.trim());
    }

    #[tokio::test]
    async fn rejects_an_unknown_tool() {
        let error = run("fly", &json!({})).await.unwrap_err();

        assert_eq!(error, "unknown tool `fly`");
    }
}
