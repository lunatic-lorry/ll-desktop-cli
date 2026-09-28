use anyhow::{Context as _, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use flags2env::BundledFlags2Env;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, env, net::IpAddr, path::PathBuf, time::Duration};
use uuid::Uuid;

const MAX_MODULE_BYTES: usize = 64 * 1024 * 1024;
const MAX_ORES_ADAPTER_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct CliConfig {
    LL_DESKTOP_DAEMON_URL: String,
    LL_DESKTOP_TIMEOUT_MS: i64,
    LL_DESKTOP_TENANT_ID: Option<String>,
    LL_DESKTOP_DEPLOYMENT_ID: Option<String>,
    LL_DESKTOP_MODULE: Option<String>,
    LL_DESKTOP_ORES_ADAPTER: Option<String>,
    LL_DESKTOP_PAYLOAD: Option<Value>,
    FLAGS2ENV_COMMAND: Option<String>,
}

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("ll-desktop: {error}");
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
    parser
        .audit_config(Some(config_path_text))
        .map_err(|error| anyhow!("flags-2-env configuration audit failed: {error}"))?;

    let argv = env::args().collect::<Vec<_>>();
    let parsed = parser
        .parse_structured(&argv, Some(config_path_text))
        .map_err(|error| anyhow!("flags-2-env parse failed: {error}"))?;
    if !parsed.unknown_options.is_empty() {
        bail!("unknown command-line options: {}", parsed.unknown_options.len());
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
    let config = parser
        .coerce::<CliConfig, _>(&raw, Some(config_path_text))
        .map_err(|error| anyhow!("flags-2-env typed configuration failed: {error}"))?;
    let timeout_ms = u64::try_from(config.LL_DESKTOP_TIMEOUT_MS)
        .ok()
        .filter(|value| *value > 0 && *value <= 1_200_000)
        .ok_or_else(|| anyhow!("--timeout must be between 1 and 1200000 ms"))?;
    let token = read_token()?;
    let base = validate_daemon_url(&config.LL_DESKTOP_DAEMON_URL)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(timeout_ms.saturating_add(5_000)))
        .build()?;

    match config.FLAGS2ENV_COMMAND.as_deref().unwrap_or("") {
        "status" => {
            let response = client
                .get(format!("{base}/v1/status"))
                .bearer_auth(&token)
                .send()
                .await?;
            print_response(response).await?;
        }
        "deploy" => {
            let tenant_id = validated_id(config.LL_DESKTOP_TENANT_ID, "--tenant")?;
            let deployment_id = validated_id(config.LL_DESKTOP_DEPLOYMENT_ID, "--deployment")?;
            let module_path = required(config.LL_DESKTOP_MODULE, "--module")?;
            let module = tokio::fs::read(&module_path)
                .await
                .with_context(|| format!("cannot read module at {module_path}"))?;
            if module.is_empty() || module.len() > MAX_MODULE_BYTES {
                bail!("module must be between 1 and {MAX_MODULE_BYTES} bytes");
            }
            let ores_adapter =
                read_optional_ores_adapter(config.LL_DESKTOP_ORES_ADAPTER.as_deref()).await?;
            let adapter_supplied = ores_adapter.is_some();
            let body = json!({
                "tenant_id": tenant_id,
                "deployment_id": deployment_id,
                "wasm_base64": BASE64.encode(&module),
                "ores_adapter": ores_adapter,
            });
            let expected_sha256 = format!("{:x}", Sha256::digest(&module));
            let response = client
                .post(format!("{base}/v1/deploy"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await?;
            let value = read_json_response(response).await?;
            validate_deploy_ack(
                &value,
                &tenant_id,
                &deployment_id,
                &expected_sha256,
                module.len(),
                adapter_supplied,
            )?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        "invoke" => {
            let tenant_id = validated_id(config.LL_DESKTOP_TENANT_ID, "--tenant")?;
            let deployment_id = validated_id(config.LL_DESKTOP_DEPLOYMENT_ID, "--deployment")?;
            let body = json!({
                "invocation_id": Uuid::new_v4().to_string(),
                "tenant_id": tenant_id,
                "deployment_id": deployment_id,
                "payload_json": config.LL_DESKTOP_PAYLOAD.unwrap_or_else(|| json!({})),
                "timeout_ms": timeout_ms,
            });
            let response = client
                .post(format!("{base}/v1/invoke"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await?;
            print_response(response).await?;
        }
        _ => {
            bail!("command required: status, deploy, or invoke");
        }
    }
    return Ok(());
}

async fn read_optional_ores_adapter(path: Option<&str>) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if path.trim().is_empty() {
        bail!("--ores-adapter must name a readable JSON file");
    }
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("cannot read ORES adapter at {path}"))?;
    return parse_ores_adapter_bytes(&bytes).map(Some);
}

fn parse_ores_adapter_bytes(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > MAX_ORES_ADAPTER_BYTES {
        bail!("ORES adapter must be between 1 and {MAX_ORES_ADAPTER_BYTES} bytes");
    }
    let value: Value = serde_json::from_slice(bytes).context("ORES adapter is not valid JSON")?;
    if !value.is_object() {
        bail!("ORES adapter must be a JSON object");
    }
    return Ok(value);
}

async fn read_json_response(mut response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        bail!("daemon response exceeds {MAX_RESPONSE_BYTES} bytes");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            bail!("daemon response exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let text = String::from_utf8_lossy(&body);
        bail!("daemon returned {status}: {text}");
    }
    return serde_json::from_slice(&body).context("daemon response was not JSON");
}

async fn print_response(response: reqwest::Response) -> Result<()> {
    let value = read_json_response(response).await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    return Ok(());
}

fn validate_deploy_ack(
    value: &Value,
    tenant_id: &str,
    deployment_id: &str,
    expected_sha256: &str,
    expected_module_bytes: usize,
    adapter_supplied: bool,
) -> Result<()> {
    if value.get("tenant_id").and_then(Value::as_str) != Some(tenant_id) {
        bail!("daemon deploy response tenant_id did not match the requested tenant");
    }
    if value.get("deployment_id").and_then(Value::as_str) != Some(deployment_id) {
        bail!("daemon deploy response deployment_id did not match the requested deployment");
    }
    if value.get("sha256").and_then(Value::as_str) != Some(expected_sha256) {
        bail!("daemon deploy response sha256 did not match the uploaded module");
    }
    let expected_module_bytes = u64::try_from(expected_module_bytes)
        .map_err(|_| anyhow!("module byte length does not fit in u64"))?;
    if value.get("module_bytes").and_then(Value::as_u64) != Some(expected_module_bytes) {
        bail!("daemon deploy response module_bytes did not match the uploaded module");
    }
    if value.get("ores_adapter_verified").and_then(Value::as_bool) != Some(adapter_supplied) {
        bail!(
            "daemon deploy response adapter verification did not match whether an ORES adapter was supplied"
        );
    }
    return Ok(());
}

fn required(value: Option<String>, flag: &str) -> Result<String> {
    return value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("{flag} is required"));
}

fn validated_id(value: Option<String>, flag: &str) -> Result<String> {
    let value = required(value, flag)?;
    let valid = value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != "..";
    if !valid {
        bail!("{flag} must contain only ASCII letters, digits, '.', '_' or '-'");
    }
    return Ok(value);
}

fn validate_daemon_url(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value).context("daemon URL is invalid")?;
    if !url.username().is_empty() || url.password().is_some() {
        bail!("daemon URL must not embed credentials");
    }
    if url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        bail!("daemon URL must be an origin without a path, query, or fragment");
    }
    if !matches!(url.scheme(), "http" | "https") {
        bail!("daemon URL scheme must be http or https");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("daemon URL must contain a host"))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if url.scheme() == "http" && !loopback {
        bail!("plain HTTP daemon URLs are allowed only for loopback; use HTTPS remotely");
    }
    return Ok(url.as_str().trim_end_matches('/').to_owned());
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
    if let Some(parent) = env::current_exe()?.parent() {
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
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("cannot inspect daemon token at {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("daemon token path must be a regular non-symlink file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("daemon token file must not be accessible by group or other users");
        }
    }
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
    use super::*;

    #[test]
    fn deploy_ack_binds_exact_requested_identity() -> Result<()> {
        let expected_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let good = json!({
            "tenant_id": "tenant-a",
            "deployment_id": "release-1",
            "sha256": expected_sha256,
            "module_bytes": 42,
            "ores_adapter_verified": true
        });
        validate_deploy_ack(&good, "tenant-a", "release-1", expected_sha256, 42, true)?;

        for field in ["tenant_id", "deployment_id", "sha256"] {
            let mut bad = good.clone();
            bad[field] = Value::String("wrong".to_owned());
            assert!(
                validate_deploy_ack(&bad, "tenant-a", "release-1", expected_sha256, 42, true,)
                    .is_err()
            );
        }

        let mut bad = good.clone();
        bad["module_bytes"] = Value::from(41_u64);
        assert!(
            validate_deploy_ack(&bad, "tenant-a", "release-1", expected_sha256, 42, true,).is_err()
        );

        let mut bad = good;
        bad["ores_adapter_verified"] = Value::Bool(false);
        assert!(
            validate_deploy_ack(&bad, "tenant-a", "release-1", expected_sha256, 42, true,).is_err()
        );
        return Ok(());
    }

    #[test]
    fn deploy_ack_without_adapter_requires_unverified_status() -> Result<()> {
        let expected_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let value = json!({
            "tenant_id": "tenant-a",
            "deployment_id": "release-2",
            "sha256": expected_sha256,
            "module_bytes": 7,
            "ores_adapter_verified": false
        });
        return validate_deploy_ack(&value, "tenant-a", "release-2", expected_sha256, 7, false);
    }

    #[test]
    fn daemon_url_rejects_remote_plaintext_and_embedded_credentials() -> Result<()> {
        assert_eq!(
            validate_daemon_url("http://127.0.0.1:8763")?,
            "http://127.0.0.1:8763"
        );
        assert!(validate_daemon_url("http://example.com:8763").is_err());
        assert!(validate_daemon_url("https://user:pass@example.com").is_err());
        assert!(validate_daemon_url("https://example.com/api").is_err());
        return Ok(());
    }

    #[test]
    fn identifiers_reject_path_traversal() -> Result<()> {
        assert_eq!(
            validated_id(Some("tenant-1.alpha".to_owned()), "--tenant")?,
            "tenant-1.alpha"
        );
        assert!(validated_id(Some("../escape".to_owned()), "--tenant").is_err());
        assert!(validated_id(Some("a/b".to_owned()), "--tenant").is_err());
        return Ok(());
    }

    #[test]
    fn ores_adapter_is_bounded_json_object() -> Result<()> {
        let value = parse_ores_adapter_bytes(
            br#"{"schema_version":"ores.lambda.adapter/v1","provider":"lunatic_lorry"}"#,
        )?;
        assert_eq!(value["provider"], "lunatic_lorry");
        assert!(parse_ores_adapter_bytes(b"[]").is_err());
        assert!(parse_ores_adapter_bytes(b"").is_err());
        return Ok(());
    }

    #[test]
    fn module_digest_is_lowercase_sha256() {
        let digest = format!("{:x}", Sha256::digest(b"lunatic-lorry"));
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(digest, digest.to_ascii_lowercase());
    }
}
