use serde_json::{Value, json};
use std::io::{self, Write};
use std::process::{Command, ExitCode};

const CREDENTIALS_SERVICE: &str = "Claude Code-credentials";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const SYSTEM_PROMPT: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const MODEL: &str = "claude-opus-5-5";
const MAX_TOKENS: u32 = 16000;
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

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
    let token = access_token(&credentials_json()?)?;
    let client = reqwest::Client::new();
    let mut messages: Vec<Value> = Vec::new();

    while let Some(prompt) = read_prompt()? {
        if prompt.is_empty() {
            continue;
        }

        messages.push(json!({"role": "user", "content": prompt}));

        match stream_reply(&client, &token, &messages).await {
            Ok(content) => messages.push(json!({"role": "assistant", "content": content})),
            Err(message) => {
                eprintln!("{RESET}error: {message}");
                messages.pop();
            }
        }
    }

    Ok(())
}

fn read_prompt() -> Result<Option<String>, String> {
    print!("> ");
    io::stdout()
        .flush()
        .map_err(|error| format!("could not write to the terminal: {error}"))?;

    let mut line = String::new();
    let read = io::stdin()
        .read_line(&mut line)
        .map_err(|error| format!("could not read input: {error}"))?;

    if read == 0 {
        println!();
        return Ok(None);
    }

    Ok(Some(line.trim().to_string()))
}

async fn stream_reply(
    client: &reqwest::Client,
    token: &str,
    messages: &[Value],
) -> Result<Vec<Value>, String> {
    let mut response = client
        .post(API_URL)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .bearer_auth(token)
        .json(&request_body(messages))
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body: Value = response
            .json()
            .await
            .map_err(|error| format!("could not read the response body: {error}"))?;
        return Err(describe_failure(status, &body));
    }

    let mut parser = EventParser::default();
    let mut reply = Reply::default();
    let mut stdout = io::stdout();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("the stream was interrupted: {error}"))?
    {
        for event in parser.push(&chunk)? {
            let output = reply.apply(&event)?;
            print!("{output}");
            stdout
                .flush()
                .map_err(|error| format!("could not write to the terminal: {error}"))?;
        }
    }
    println!();

    reply.finish()
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

fn request_body(messages: &[Value]) -> Value {
    json!({
        "model": MODEL,
        "max_tokens": MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        "stream": true,
        "cache_control": {"type": "ephemeral"},
        "thinking": {"type": "adaptive", "display": "summarized"},
        "messages": messages,
    })
}

#[derive(Default)]
struct EventParser {
    buffer: Vec<u8>,
}

impl EventParser {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some(position) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=position).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(data) = line.trim_end().strip_prefix("data:") {
                let event = serde_json::from_str(data.trim_start())
                    .map_err(|error| format!("stream event is not valid JSON: {error}"))?;
                events.push(event);
            }
        }

        Ok(events)
    }
}

#[derive(Default)]
struct Reply {
    content: Vec<Value>,
}

impl Reply {
    fn apply(&mut self, event: &Value) -> Result<String, String> {
        let event_type = event.get("type").and_then(Value::as_str);
        let delta = event.get("delta");
        let delta_type = delta
            .and_then(|delta| delta.get("type"))
            .and_then(Value::as_str);

        match (event_type, delta_type) {
            (Some("content_block_start"), _) => {
                let block = event.get("content_block").cloned().unwrap_or(Value::Null);
                let output = match block.get("type").and_then(Value::as_str) {
                    Some("thinking") => DIM,
                    _ => "",
                };
                self.content.push(block);
                Ok(output.to_string())
            }
            (Some("content_block_delta"), Some("thinking_delta")) => {
                Ok(self.append(delta, "thinking"))
            }
            (Some("content_block_delta"), Some("text_delta")) => Ok(self.append(delta, "text")),
            (Some("content_block_delta"), Some("signature_delta")) => {
                let signature = delta_field(delta, "signature");
                if let Some(block) = self.content.last_mut() {
                    block["signature"] = Value::String(signature);
                }
                Ok(String::new())
            }
            (Some("content_block_stop"), _) => {
                match self.content.last().and_then(|block| block.get("type")) {
                    Some(block_type) if block_type == "thinking" => Ok(format!("{RESET}\n\n")),
                    _ => Ok(String::new()),
                }
            }
            (Some("error"), _) => Err(describe_failure(0, event)),
            _ => Ok(String::new()),
        }
    }

    fn append(&mut self, delta: Option<&Value>, field: &str) -> String {
        let text = delta_field(delta, field);
        if let Some(block) = self.content.last_mut() {
            let joined = format!("{}{text}", block[field].as_str().unwrap_or_default());
            block[field] = Value::String(joined);
        }
        text
    }

    fn finish(self) -> Result<Vec<Value>, String> {
        let has_text = self.content.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
        });

        if !has_text {
            return Err("response contained no text".to_string());
        }

        Ok(self.content)
    }
}

fn delta_field(delta: Option<&Value>, field: &str) -> String {
    delta
        .and_then(|delta| delta.get(field))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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

    match status {
        0 => format!("stream failed with {error_type}: {message}"),
        _ => format!("request failed with {status} {error_type}: {message}{hint}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREDENTIALS: &str =
        r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-test","subscriptionType":"team"}}"#;

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
    fn sends_the_conversation_as_a_cached_stream_with_visible_thinking() {
        let messages = vec![
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "content": [{"type": "text", "text": "hello"}]}),
            json!({"role": "user", "content": "how are you?"}),
        ];
        let body = request_body(&messages);
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["system"], SYSTEM_PROMPT);
        assert_eq!(body["stream"], true);
        assert_eq!(body["cache_control"]["type"], "ephemeral");
        assert_eq!(body["thinking"]["display"], "summarized");
        assert_eq!(body["messages"], json!(messages));
    }

    #[test]
    fn parses_events_split_across_chunks() {
        let stream = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"héllo\"}}\n\nevent: ping\ndata: {\"type\":\"ping\"}\n\n";
        let bytes = stream.as_bytes();
        let split = stream.find('é').unwrap() + 1;
        let mut parser = EventParser::default();

        let first = parser.push(&bytes[..split]).unwrap();
        let rest = parser.push(&bytes[split..]).unwrap();

        assert!(first.is_empty());
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0]["delta"]["text"], "héllo");
        assert_eq!(rest[1]["type"], "ping");
    }

    #[test]
    fn shows_thinking_dimmed_and_keeps_every_block_for_the_next_turn() {
        let events = [
            json!({"type": "message_start"}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hm"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "m"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "redacted_thinking", "data": "secret"}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "hel"}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "lo"}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "message_stop"}),
        ];
        let mut reply = Reply::default();

        let output: String = events
            .iter()
            .map(|event| reply.apply(event).unwrap())
            .collect();

        assert_eq!(output, format!("{DIM}hmm{RESET}\n\nhello"));
        assert_eq!(
            reply.finish().unwrap(),
            vec![
                json!({"type": "thinking", "thinking": "hmm", "signature": "sig"}),
                json!({"type": "redacted_thinking", "data": "secret"}),
                json!({"type": "text", "text": "hello"}),
            ]
        );
    }

    #[test]
    fn reports_a_reply_without_text() {
        let mut reply = Reply::default();
        reply
            .apply(&json!({"type": "content_block_start", "content_block": {"type": "text", "text": ""}}))
            .unwrap();
        let error = reply.finish().unwrap_err();
        assert!(error.contains("no text"), "{error}");
    }

    #[test]
    fn reports_an_error_in_the_stream() {
        let event =
            json!({"type": "error", "error": {"type": "overloaded_error", "message": "busy"}});
        let error = Reply::default().apply(&event).unwrap_err();
        assert_eq!(error, "stream failed with overloaded_error: busy");
    }

    #[test]
    fn explains_a_rejected_token() {
        let body = json!({"error": {"type": "authentication_error", "message": "invalid"}});
        let message = describe_failure(401, &body);
        assert!(message.contains("401"), "{message}");
        assert!(message.contains("run `claude`"), "{message}");
    }
}
