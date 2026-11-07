use serde_json::{Value, json};
use std::process::{Command, ExitCode};

const CREDENTIALS_SERVICE: &str = "Claude Code-credentials";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const SYSTEM_PROMPT: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const DEFAULT_MODEL: &str = "claude-opus-5-5";
const PROMPT: &str = "Say hello.";
const MAX_TOKENS: u32 = 1024;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let model = parse_model_flag(&arguments)?;
    let token = access_token(&credentials_json()?)?;

    let response = reqwest::Client::new()
        .post(API_URL)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .bearer_auth(&token)
        .json(&request_body(&model))
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("could not read the response body: {error}"))?;

    if !(200..300).contains(&status) {
        return Err(describe_failure(status, &body));
    }

    println!("{}", response_text(&body)?);
    Ok(())
}

fn credentials_json() -> Result<String, String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", CREDENTIALS_SERVICE, "-w"])
        .output()
        .map_err(|error| format!("could not run `security`: {error}"))?;

    if !output.status.success() {
        return Err(
            "no Claude Code credentials in the keychain; run `claude` to sign in".to_string(),
        );
    }

    String::from_utf8(output.stdout)
        .map_err(|error| format!("keychain output was not text: {error}"))
}

fn access_token(credentials: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(credentials)
        .map_err(|error| format!("credentials are not valid JSON: {error}"))?;

    value
        .get("claudeAiOauth")
        .and_then(|oauth| oauth.get("accessToken"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            "credentials have no claudeAiOauth.accessToken; run `claude` to sign in".to_string()
        })
}

fn request_body(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        "messages": [{"role": "user", "content": PROMPT}],
    })
}

fn response_text(body: &Value) -> Result<String, String> {
    let blocks = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| "response has no content blocks".to_string())?;

    let text = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<&str>>()
        .join("\n");

    if text.is_empty() {
        return Err("response contained no text".to_string());
    }

    Ok(text)
}

fn describe_failure(status: u16, body: &Value) -> String {
    let error_type = body
        .get("error")
        .and_then(|error| error.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown_error");
    let message = body
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("no message");

    let hint = match status {
        401 | 403 => "; run `claude` to sign in again",
        _ => "",
    };

    format!("request failed with {status} {error_type}: {message}{hint}")
}

fn parse_model_flag(arguments: &[String]) -> Result<String, String> {
    let mut model = None;
    let mut index = 0;

    while index < arguments.len() {
        let argument = &arguments[index];

        if argument == "--model" {
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| "--model needs a value".to_string())?;
            model = Some(value.clone());
            index += 2;
        } else if let Some(value) = argument.strip_prefix("--model=") {
            model = Some(value.to_string());
            index += 1;
        } else {
            return Err(format!("unknown argument: {argument}"));
        }
    }

    if model.as_deref() == Some("") {
        return Err("--model cannot be empty".to_string());
    }

    Ok(model.unwrap_or_else(|| DEFAULT_MODEL.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREDENTIALS: &str =
        r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-test","subscriptionType":"team"}}"#;

    fn arguments(list: &[&str]) -> Vec<String> {
        list.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn reads_the_access_token() {
        assert_eq!(access_token(CREDENTIALS).unwrap(), "sk-ant-oat01-test");
    }

    #[test]
    fn rejects_credentials_without_a_token() {
        let error = access_token(r#"{"claudeAiOauth":{}}"#).unwrap_err();
        assert!(error.contains("claudeAiOauth.accessToken"), "{error}");
    }

    #[test]
    fn rejects_credentials_that_are_not_json() {
        let error = access_token("not json").unwrap_err();
        assert!(error.contains("not valid JSON"), "{error}");
    }

    #[test]
    fn sends_the_claude_code_system_prompt() {
        let body = request_body("claude-opus-5-5");
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["system"], SYSTEM_PROMPT);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], PROMPT);
    }

    #[test]
    fn joins_text_blocks_and_skips_others() {
        let body = json!({"content": [
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "hello"},
            {"type": "text", "text": "world"},
        ]});
        assert_eq!(response_text(&body).unwrap(), "hello\nworld");
    }

    #[test]
    fn reports_a_response_without_text() {
        let error = response_text(&json!({"content": []})).unwrap_err();
        assert!(error.contains("no text"), "{error}");
    }

    #[test]
    fn explains_a_rejected_token() {
        let body = json!({"error": {"type": "authentication_error", "message": "invalid"}});
        let message = describe_failure(401, &body);
        assert!(message.contains("401"), "{message}");
        assert!(message.contains("run `claude`"), "{message}");
    }

    #[test]
    fn defaults_the_model() {
        assert_eq!(parse_model_flag(&[]).unwrap(), DEFAULT_MODEL);
    }

    #[test]
    fn reads_the_model_flag() {
        assert_eq!(
            parse_model_flag(&arguments(&["--model", "claude-haiku-5-5"])).unwrap(),
            "claude-haiku-5-5"
        );
        assert_eq!(
            parse_model_flag(&arguments(&["--model=claude-sonnet-5-5"])).unwrap(),
            "claude-sonnet-5-5"
        );
    }

    #[test]
    fn rejects_a_model_flag_without_a_value() {
        assert!(parse_model_flag(&arguments(&["--model"])).is_err());
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert!(parse_model_flag(&arguments(&["--verbose"])).is_err());
    }
}
