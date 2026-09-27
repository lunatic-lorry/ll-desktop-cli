use anyhow::{Context as _, Result, anyhow, bail};
use flags2env::BundledFlags2Env;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    env,
    net::IpAddr,
    path::PathBuf,
    time::Duration,
};
use uuid::Uuid;

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct CliConfig {
    LL_DESKTOP_DAEMON_URL: String,
    LL_DESKTOP_TIMEOUT_MS: i64,
    LL_DESKTOP_TENANT_ID: Option<String>,
    LL_DESKTOP_DEPLOYMENT_ID: Option<String>,
    LL_DESKTOP_PAYLOAD: Option<Value>,
    FLAGS2ENV_COMMAND: Option<String>,
}

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("ll-desktop-cli: {error}");
            2
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<()> {
    let config_path = resolve_config_path()?;
    let config_path_text = config_path
        .to_str()
        .ok_or_else(|| anyhow!(".cli-flags.toml path is not UTF-8"))?;
    let parser = BundledFlags2Env::new();
    parser.audit_config(Some(config_path_text))?;

    let argv = env::args().collect::<Vec<_>>();
    let parsed = parser.parse_structured(&argv, Some(config_path_text))?;
    if !parsed.unknown_options.is_empty() {
        bail!(
            "unknown command-line options: {}",
            parsed.unknown_options.len()
        );
    }
    if !parsed.errors.is_empty() {
        bail!("invalid command-line values: {}", parsed.errors.join("; "));
    }
    if !parsed.extras.is_empty() {
        bail!("unexpected positional arguments: {}", parsed.extras.len());
    }

    let mut raw = env::vars().collect::<HashMap<_, _>>();
    raw.remove("FLAGS2ENV_COMMAND");
    raw.extend(parsed.provided_flags);
    let config = parser.coerce::<CliConfig, _>(&raw, Some(config_path_text))?;
    let command = config.FLAGS2ENV_COMMAND.as_deref().unwrap_or("");
    let timeout_ms = u64::try_from(config.LL_DESKTOP_TIMEOUT_MS)
        .ok()
        .filter(|value| *value > 0 && *value <= 1_200_000)
        .ok_or_else(|| anyhow!("--timeout must be between 1 and 1200000 ms"))?;
    let daemon_url = validate_daemon_url(&config.LL_DESKTOP_DAEMON_URL)?;
    let token = read_token()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms.saturating_add(2_000)))
        .build()?;

    match command {
        "status" => {
            let response = client
                .get(format!("{daemon_url}/v1/status"))
                .bearer_auth(&token)
                .send()
                .await?;
            print_response(response).await?;
        }
        "invoke" => {
            let tenant_id = required(config.LL_DESKTOP_TENANT_ID, "--tenant")?;
            let deployment_id = required(config.LL_DESKTOP_DEPLOYMENT_ID, "--deployment")?;
            let payload_json = config.LL_DESKTOP_PAYLOAD.unwrap_or_else(|| json!({}));
            let body = json!({
                "invocation_id": Uuid::new_v4().to_string(),
                "tenant_id": tenant_id,
                "deployment_id": deployment_id,
                "payload_json": payload_json,
                "timeout_ms": timeout_ms,
            });
            let response = client
                .post(format!("{daemon_url}/v1/invoke"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await?;
            print_response(response).await?;
        }
        _ => {
            bail!("command required: status or invoke");
        }
    }

    return Ok(());
}

async fn print_response(response: reqwest::Response) -> Result<()> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("daemon returned {status}: {body}");
    }
    let value: Value = serde_json::from_str(&body).context("daemon response was not JSON")?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    return Ok(());
}

fn required(value: Option<String>, flag: &str) -> Result<String> {
    return value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("{flag} is required"));
}

fn validate_daemon_url(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value).context("daemon URL is invalid")?;
    if url.scheme() != "http" {
        bail!("daemon URL must use http on loopback");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("daemon URL must not contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("daemon URL must not contain query or fragment components");
    }
    if url.path() != "/" && !url.path().is_empty() {
        bail!("daemon URL must be an origin without a path");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("daemon URL must include a host"))?;
    let ip = host
        .parse::<IpAddr>()
        .context("daemon URL host must be a numeric loopback address")?;
    if !ip.is_loopback() {
        bail!("daemon URL host must be loopback");
    }
    return Ok(value.trim_end_matches('/').to_owned());
}

fn resolve_config_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("LL_DESKTOP_FLAGS_CONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!("LL_DESKTOP_FLAGS_CONFIG is not a readable file");
    }

    let current = env::current_dir()?.join(".cli-flags.toml");
    if current.is_file() {
        return Ok(current);
    }

    let executable = env::current_exe()?;
    if let Some(parent) = executable.parent() {
        let adjacent = parent.join(".cli-flags.toml");
        if adjacent.is_file() {
            return Ok(adjacent);
        }
    }

    bail!("cannot locate .cli-flags.toml");
}

fn read_token() -> Result<String> {
    let path = if let Some(path) = env::var_os("LL_DESKTOP_TOKEN_FILE") {
        PathBuf::from(path)
    } else {
        let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME is required"))?;
        PathBuf::from(home).join(".lunatic-lorry/daemon/token")
    };
    let token = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read daemon token at {}", path.display()))?;
    let token = token.trim();
    if token.len() < 32 {
        bail!("daemon token is invalid");
    }
    return Ok(token.to_owned());
}

#[cfg(test)]
mod tests {
    use super::validate_daemon_url;

    #[test]
    fn accepts_numeric_loopback_origins() {
        assert_eq!(
            validate_daemon_url("http://127.0.0.1:8763").as_deref(),
            Ok("http://127.0.0.1:8763")
        );
        assert_eq!(
            validate_daemon_url("http://[::1]:8763/").as_deref(),
            Ok("http://[::1]:8763")
        );
    }

    #[test]
    fn rejects_non_loopback_or_credentialed_origins() {
        assert!(validate_daemon_url("https://127.0.0.1:8763").is_err());
        assert!(validate_daemon_url("http://192.0.2.10:8763").is_err());
        assert!(validate_daemon_url("http://localhost:8763").is_err());
        assert!(validate_daemon_url("http://user:secret@127.0.0.1:8763").is_err());
        assert!(validate_daemon_url("http://127.0.0.1:8763/path").is_err());
    }
}
