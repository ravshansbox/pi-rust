use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const KEYCHAIN_SERVICE: &str = "pi-rust";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const SCOPES: &str = "user:profile user:inference";
const EXPIRY_MARGIN_MILLISECONDS: u64 = 5 * 60 * 1000;

#[derive(Debug, PartialEq)]
pub struct Credentials {
    access: String,
    refresh: String,
    expires: u64,
}

impl Credentials {
    fn from_response(response: &Value, now: u64) -> Result<Self, String> {
        let text = |key: &str| {
            response
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("token response has no {key}: {response}"))
        };
        let access = text("access_token")?;
        let refresh = text("refresh_token")?;
        let expires_in = response
            .get("expires_in")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("token response has no expires_in: {response}"))?;

        Ok(Self {
            access,
            refresh,
            expires: now + expires_in * 1000 - EXPIRY_MARGIN_MILLISECONDS,
        })
    }

    fn parse(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text)
            .map_err(|error| format!("stored credentials are not valid JSON: {error}"))?;
        let field = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("stored credentials have no {key}"))
        };

        Ok(Self {
            access: field("access")?,
            refresh: field("refresh")?,
            expires: value
                .get("expires")
                .and_then(Value::as_u64)
                .ok_or("stored credentials have no expires")?,
        })
    }

    fn to_json(&self) -> String {
        json!({"access": self.access, "refresh": self.refresh, "expires": self.expires}).to_string()
    }

    fn has_expired(&self, now: u64) -> bool {
        now >= self.expires
    }
}

pub async fn credentials(client: &reqwest::Client) -> Result<Credentials, String> {
    match load()? {
        Some(credentials) => Ok(credentials),
        None => sign_in(client).await,
    }
}

pub async fn access_token(
    client: &reqwest::Client,
    credentials: &mut Credentials,
) -> Result<String, String> {
    if credentials.has_expired(now()) {
        let response = request_tokens(
            client,
            json!({
                "grant_type": "refresh_token",
                "client_id": CLIENT_ID,
                "refresh_token": credentials.refresh,
            }),
        )
        .await?;
        *credentials = Credentials::from_response(&response, now())?;
        save(credentials)?;
    }

    Ok(credentials.access.clone())
}

async fn sign_in(client: &reqwest::Client) -> Result<Credentials, String> {
    let verifier = verifier()?;
    let url = sign_in_url(&verifier);
    println!("sign in to Claude in your browser, then paste the code here:\n{url}");
    let _ = Command::new("open").arg(url.as_str()).status();

    let input = crate::read_prompt()?.ok_or("sign-in cancelled")?;
    let code = pasted_code(&input, &verifier)?;
    let response = request_tokens(
        client,
        json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": verifier,
            "redirect_uri": REDIRECT_URI,
            "code_verifier": verifier,
        }),
    )
    .await?;

    let credentials = Credentials::from_response(&response, now())?;
    save(&credentials)?;
    Ok(credentials)
}

async fn request_tokens(client: &reqwest::Client, body: Value) -> Result<Value, String> {
    let response = client
        .post(TOKEN_URL)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("token request failed: {error}"))?;

    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|error| format!("could not read the token response: {error}"))?;

    if !(200..300).contains(&status) {
        return Err(format!(
            "token request failed with {status}: {text}; run `security delete-generic-password -s {KEYCHAIN_SERVICE}` to sign in again"
        ));
    }

    serde_json::from_str(&text)
        .map_err(|error| format!("token response is not valid JSON: {error}"))
}

fn verifier() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("could not read random bytes: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn sign_in_url(verifier: &str) -> reqwest::Url {
    reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        [
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPES),
            ("code_challenge", challenge(verifier).as_str()),
            ("code_challenge_method", "S256"),
            ("state", verifier),
        ],
    )
    .expect("the authorise URL is valid")
}

fn pasted_code(input: &str, verifier: &str) -> Result<String, String> {
    let input = input.trim();
    let (code, state) = input.split_once('#').unwrap_or((input, verifier));

    if code.is_empty() {
        return Err("no code was pasted".to_string());
    }
    if state != verifier {
        return Err("the pasted code does not match this sign-in".to_string());
    }

    Ok(code.to_string())
}

fn load() -> Result<Option<Credentials>, String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .map_err(|error| format!("could not run `security`: {error}"))?;

    if !output.status.success() {
        return Ok(None);
    }

    let text = String::from_utf8(output.stdout)
        .map_err(|error| format!("keychain output was not text: {error}"))?;
    Credentials::parse(text.trim()).map(Some)
}

fn save(credentials: &Credentials) -> Result<(), String> {
    let status = Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            KEYCHAIN_SERVICE,
            "-s",
            KEYCHAIN_SERVICE,
            "-w",
            &credentials.to_json(),
        ])
        .status()
        .map_err(|error| format!("could not run `security`: {error}"))?;

    if !status.success() {
        return Err("could not save credentials to the keychain".to_string());
    }

    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_the_pkce_challenge_from_the_verifier() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn makes_a_verifier_of_random_url_safe_text() {
        let first = verifier().unwrap();
        let second = verifier().unwrap();
        assert_eq!(first.len(), 43);
        assert_ne!(first, second);
        assert!(
            first
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character)),
            "{first}"
        );
    }

    #[test]
    fn builds_the_sign_in_url() {
        let url = sign_in_url("the-verifier");
        let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();

        assert_eq!(url.as_str().split('?').next(), Some(AUTHORIZE_URL));
        for (key, value) in [
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPES),
            ("code_challenge", challenge("the-verifier").as_str()),
            ("code_challenge_method", "S256"),
            ("state", "the-verifier"),
        ] {
            assert!(
                query.contains(&(key.to_string(), value.to_string())),
                "{key} missing from {url}"
            );
        }
    }

    #[test]
    fn reads_the_pasted_code() {
        assert_eq!(
            pasted_code(" abc#the-verifier\n", "the-verifier").unwrap(),
            "abc"
        );
        assert_eq!(pasted_code("abc", "the-verifier").unwrap(), "abc");
    }

    #[test]
    fn rejects_a_code_from_another_sign_in() {
        let error = pasted_code("abc#other", "the-verifier").unwrap_err();
        assert!(error.contains("does not match"), "{error}");
    }

    #[test]
    fn rejects_an_empty_code() {
        let error = pasted_code("  ", "the-verifier").unwrap_err();
        assert!(error.contains("no code"), "{error}");
    }

    #[test]
    fn reads_tokens_and_expires_them_five_minutes_early() {
        let response =
            json!({"access_token": "access", "refresh_token": "refresh", "expires_in": 3600});

        let credentials = Credentials::from_response(&response, 1_000).unwrap();

        assert_eq!(
            credentials,
            Credentials {
                access: "access".to_string(),
                refresh: "refresh".to_string(),
                expires: 1_000 + 3_300_000,
            }
        );
    }

    #[test]
    fn rejects_a_token_response_without_tokens() {
        let error = Credentials::from_response(&json!({}), 0).unwrap_err();
        assert!(error.contains("access_token"), "{error}");
    }

    #[test]
    fn keeps_credentials_between_runs() {
        let credentials = Credentials {
            access: "access".to_string(),
            refresh: "refresh".to_string(),
            expires: 42,
        };

        assert_eq!(
            Credentials::parse(&credentials.to_json()).unwrap(),
            credentials
        );
    }

    #[test]
    fn knows_when_the_access_token_has_expired() {
        let credentials = Credentials {
            access: "access".to_string(),
            refresh: "refresh".to_string(),
            expires: 100,
        };

        assert!(!credentials.has_expired(99));
        assert!(credentials.has_expired(100));
    }
}
