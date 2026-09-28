use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MAX_ORES_RECEIPT_BYTES: usize = 1024 * 1024;
const RECEIPT_SCHEMA: &str = "ores.lambda.wasm-artifact.receipt/v1";
const ADAPTER_SCHEMA: &str = "ores.lambda.adapter/v1";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WasmArtifactReceipt {
    schema_version: String,
    generated_by: String,
    provider: String,
    runtime_repository: String,
    runtime_contract: String,
    adapter_contract: String,
    artifact_path: String,
    artifact_sha256: String,
    deploy_mutation_performed: bool,
    target_triple: String,
    adapter_path: String,
    adapter_sha256: String,
    source_path: String,
    source_sha256: String,
    cargo_manifest_path: String,
    cargo_package: String,
    cargo_target_name: String,
    wrapper_sha256: String,
    unit_manifest_sha256: String,
}

pub struct CheckedOresEvidence {
    pub adapter_bytes: Vec<u8>,
    pub receipt_bytes: Vec<u8>,
}

pub fn validate(
    adapter_bytes: Vec<u8>,
    receipt_bytes: Vec<u8>,
    module_bytes: &[u8],
    expected_provider: &str,
    expected_target: &str,
) -> Result<CheckedOresEvidence> {
    if receipt_bytes.is_empty() || receipt_bytes.len() > MAX_ORES_RECEIPT_BYTES {
        bail!("ORES receipt must be between 1 and {MAX_ORES_RECEIPT_BYTES} bytes");
    }
    let adapter_value: Value =
        serde_json::from_slice(&adapter_bytes).context("ORES adapter is not valid JSON")?;
    let adapter = adapter_value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("ORES adapter must be a JSON object"))?;
    let receipt: WasmArtifactReceipt =
        serde_json::from_slice(&receipt_bytes).context("ORES WASM receipt is not valid JSON")?;

    require(
        &receipt.schema_version,
        RECEIPT_SCHEMA,
        "receipt schema_version",
    )?;
    require(&receipt.generated_by, "ores-stack", "receipt generated_by")?;
    require(&receipt.provider, expected_provider, "receipt provider")?;
    require(
        &receipt.target_triple,
        expected_target,
        "receipt target_triple",
    )?;
    require(
        &receipt.adapter_contract,
        ADAPTER_SCHEMA,
        "receipt adapter_contract",
    )?;
    if receipt.deploy_mutation_performed {
        bail!("ORES WASM artifact receipt must be immutable build evidence");
    }

    let adapter_schema = field(adapter, "schema_version")?;
    let adapter_provider = field(adapter, "provider")?;
    let adapter_runtime_repository = field(adapter, "runtime_repository")?;
    let adapter_runtime_contract = field(adapter, "runtime_contract")?;
    let adapter_source = field(adapter, "source")?;
    let adapter_source_sha256 = field(adapter, "source_sha256")?;

    require(adapter_schema, ADAPTER_SCHEMA, "adapter schema_version")?;
    require(adapter_provider, expected_provider, "adapter provider")?;
    require(
        &receipt.provider,
        adapter_provider,
        "receipt/adapter provider",
    )?;
    require(
        &receipt.runtime_repository,
        adapter_runtime_repository,
        "receipt/adapter runtime_repository",
    )?;
    require(
        &receipt.runtime_contract,
        adapter_runtime_contract,
        "receipt/adapter runtime_contract",
    )?;
    require(
        &receipt.source_path,
        adapter_source,
        "receipt/adapter source",
    )?;
    require(
        &receipt.source_sha256,
        adapter_source_sha256,
        "receipt/adapter source_sha256",
    )?;

    let module_sha256 = sha256_hex(module_bytes);
    require(
        &receipt.artifact_sha256,
        &module_sha256,
        "receipt/module artifact_sha256",
    )?;
    let adapter_sha256 = sha256_hex(&adapter_bytes);
    require(
        &receipt.adapter_sha256,
        &adapter_sha256,
        "receipt/raw-adapter adapter_sha256",
    )?;

    for (label, digest) in [
        ("artifact_sha256", receipt.artifact_sha256.as_str()),
        ("adapter_sha256", receipt.adapter_sha256.as_str()),
        ("source_sha256", receipt.source_sha256.as_str()),
        ("wrapper_sha256", receipt.wrapper_sha256.as_str()),
        (
            "unit_manifest_sha256",
            receipt.unit_manifest_sha256.as_str(),
        ),
    ] {
        validate_sha256(digest, label)?;
    }
    for (label, value) in [
        ("artifact_path", receipt.artifact_path.as_str()),
        ("adapter_path", receipt.adapter_path.as_str()),
        ("cargo_manifest_path", receipt.cargo_manifest_path.as_str()),
        ("cargo_package", receipt.cargo_package.as_str()),
        ("cargo_target_name", receipt.cargo_target_name.as_str()),
    ] {
        if value.trim().is_empty() || value.contains('\0') {
            bail!("ORES receipt {label} must be non-empty");
        }
    }

    return Ok(CheckedOresEvidence {
        adapter_bytes,
        receipt_bytes,
    });
}

fn field<'a>(object: &'a serde_json::Map<String, Value>, name: &str) -> Result<&'a str> {
    return object
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("ORES adapter field {name} must be a string"));
}

fn require(actual: &str, expected: &str, label: &str) -> Result<()> {
    if actual != expected {
        bail!("{label} mismatch: expected {expected:?}, got {actual:?}");
    }
    return Ok(());
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    let valid = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if !valid {
        bail!("ORES receipt {label} must be lowercase SHA-256");
    }
    return Ok(());
}

fn sha256_hex(bytes: &[u8]) -> String {
    return format!("{:x}", Sha256::digest(bytes));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter_bytes() -> Vec<u8> {
        return br#"{"schema_version":"ores.lambda.adapter/v1","generated_by":"ores-stack","provider":"lunatic_lorry","runtime_repository":"lunatic-lorry/ll-lambdas","runtime_contract":"lunatic-lorry.lambda-runtime/v1","execution_boundary":"lunatic_process","isolation_model":"fresh_wasm_actor_per_invocation","artifact_kind":"wasm_module","module_cache_policy":"module_bytes_or_compiled_module_allowed","invocation_instance_reuse":"forbidden","ambient_import_policy":"explicit_lunatic_capabilities_only","durable_state":"external_only","source":"src/routes/echo/lambda.rs","source_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}"#.to_vec();
    }

    fn receipt_bytes(adapter: &[u8], module: &[u8]) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema_version": RECEIPT_SCHEMA,
            "generated_by": "ores-stack",
            "provider": "lunatic_lorry",
            "runtime_repository": "lunatic-lorry/ll-lambdas",
            "runtime_contract": "lunatic-lorry.lambda-runtime/v1",
            "adapter_contract": ADAPTER_SCHEMA,
            "artifact_path": "build/lambda/lunatic_lorry/module.wasm",
            "artifact_sha256": sha256_hex(module),
            "deploy_mutation_performed": false,
            "target_triple": "wasm32-wasip1",
            "adapter_path": "generated/lambda-adapters/lunatic_lorry/adapter.json",
            "adapter_sha256": sha256_hex(adapter),
            "source_path": "src/routes/echo/lambda.rs",
            "source_sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "cargo_manifest_path": "Cargo.toml",
            "cargo_package": "fixture",
            "cargo_target_name": "ores_wasm_lambda_unit",
            "wrapper_sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "unit_manifest_sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        }))?;
        return Ok(bytes);
    }

    #[test]
    fn exact_raw_adapter_and_module_are_bound_by_receipt() -> Result<()> {
        let adapter = adapter_bytes();
        let module = b"wasm-module";
        let receipt = receipt_bytes(&adapter, module)?;
        let checked = validate(
            adapter.clone(),
            receipt.clone(),
            module,
            "lunatic_lorry",
            "wasm32-wasip1",
        )?;
        assert_eq!(checked.adapter_bytes, adapter);
        assert_eq!(checked.receipt_bytes, receipt);
        return Ok(());
    }

    #[test]
    fn receipt_rejects_module_or_raw_adapter_drift() -> Result<()> {
        let adapter = adapter_bytes();
        let receipt = receipt_bytes(&adapter, b"wasm-module")?;
        assert!(
            validate(
                adapter.clone(),
                receipt.clone(),
                b"different-module",
                "lunatic_lorry",
                "wasm32-wasip1",
            )
            .is_err()
        );
        let mut reformatted = adapter;
        reformatted.push(b'\n');
        assert!(
            validate(
                reformatted,
                receipt,
                b"wasm-module",
                "lunatic_lorry",
                "wasm32-wasip1",
            )
            .is_err()
        );
        return Ok(());
    }
}
