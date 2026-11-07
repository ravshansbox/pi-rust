use serde_json::{Value, json};
use std::fs;

const OUTPUT_LIMIT: usize = 50_000;

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
        }
    ])
}

pub fn summary(name: &str, input: &Value) -> String {
    let detail = match name {
        "read" => input.get("path"),
        _ => None,
    }
    .and_then(Value::as_str)
    .unwrap_or_default();
    format!("{name} {detail}")
}

pub async fn run(name: &str, input: &Value) -> Result<String, String> {
    let result = match name {
        "read" => read(input),
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
    use std::path::{Path, PathBuf};

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
    async fn rejects_an_unknown_tool() {
        let error = run("fly", &json!({})).await.unwrap_err();

        assert_eq!(error, "unknown tool `fly`");
    }
}
