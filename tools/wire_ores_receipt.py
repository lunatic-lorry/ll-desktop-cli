from pathlib import Path

main_path = Path("src/main.rs")
text = main_path.read_text()


def replace_once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    text = text.replace(old, new, 1)


replace_once(
    "use anyhow::{Context as _, Result, anyhow, bail};",
    "mod ores_evidence;\n\nuse anyhow::{Context as _, Result, anyhow, bail};",
    "module declaration",
)
replace_once(
    "    LL_DESKTOP_ORES_ADAPTER: Option<String>,\n    LL_DESKTOP_PAYLOAD: Option<Value>,",
    "    LL_DESKTOP_ORES_ADAPTER: Option<String>,\n    LL_DESKTOP_ORES_RECEIPT: Option<String>,\n    LL_DESKTOP_PAYLOAD: Option<Value>,",
    "typed receipt flag",
)
replace_once(
    '''            let ores_adapter =\n                read_optional_ores_adapter(config.LL_DESKTOP_ORES_ADAPTER.as_deref()).await?;\n            let body = json!({\n                "tenant_id": tenant_id,\n                "deployment_id": deployment_id,\n                "wasm_base64": BASE64.encode(&module),\n                "ores_adapter": ores_adapter,\n            });''',
    '''            let ores_adapter_path = config.LL_DESKTOP_ORES_ADAPTER.as_deref();\n            let ores_adapter = read_optional_ores_adapter(ores_adapter_path).await?;\n            let ores_evidence = read_optional_ores_evidence(\n                ores_adapter_path,\n                config.LL_DESKTOP_ORES_RECEIPT.as_deref(),\n                &module,\n            )\n            .await?;\n            let mut body = json!({\n                "tenant_id": tenant_id,\n                "deployment_id": deployment_id,\n                "wasm_base64": BASE64.encode(&module),\n                "ores_adapter": ores_adapter,\n            });\n            if let Some(evidence) = ores_evidence {\n                body["ores_adapter_raw_base64"] =\n                    Value::String(BASE64.encode(&evidence.adapter_bytes));\n                body["ores_receipt_raw_base64"] =\n                    Value::String(BASE64.encode(&evidence.receipt_bytes));\n            }''',
    "deploy evidence forwarding",
)
marker = '''fn parse_ores_adapter_bytes(bytes: &[u8]) -> Result<Value> {'''
helper = '''async fn read_optional_ores_evidence(\n    adapter_path: Option<&str>,\n    receipt_path: Option<&str>,\n    module_bytes: &[u8],\n) -> Result<Option<ores_evidence::CheckedOresEvidence>> {\n    let Some(receipt_path) = receipt_path else {\n        return Ok(None);\n    };\n    let adapter_path = adapter_path\n        .filter(|value| !value.trim().is_empty())\n        .ok_or_else(|| anyhow!("--ores-receipt requires --ores-adapter"))?;\n    if receipt_path.trim().is_empty() {\n        bail!("--ores-receipt must name a readable JSON file");\n    }\n    let adapter_bytes = tokio::fs::read(adapter_path)\n        .await\n        .with_context(|| format!("cannot read ORES adapter at {adapter_path}"))?;\n    if adapter_bytes.is_empty() || adapter_bytes.len() > MAX_ORES_ADAPTER_BYTES {\n        bail!("ORES adapter must be between 1 and {MAX_ORES_ADAPTER_BYTES} bytes");\n    }\n    let receipt_bytes = tokio::fs::read(receipt_path)\n        .await\n        .with_context(|| format!("cannot read ORES receipt at {receipt_path}"))?;\n    let evidence = ores_evidence::validate(\n        adapter_bytes,\n        receipt_bytes,\n        module_bytes,\n        "lunatic_lorry",\n        "wasm32-wasip1",\n    )?;\n    return Ok(Some(evidence));\n}\n\n'''
replace_once(marker, helper + marker, "evidence reader insertion")
main_path.write_text(text)

flags_path = Path(".cli-flags.toml")
flags = flags_path.read_text()
old = '''[commands.deploy.flags.ores-adapter]\nenv = "LL_DESKTOP_ORES_ADAPTER"\naliases = ["ores-adapter"]\ntype = "string"\nhelp = "Optional ores.lambda.adapter/v1 JSON manifest to verify at deployment admission."\n'''
new = old + '''\n[commands.deploy.flags.ores-receipt]\nenv = "LL_DESKTOP_ORES_RECEIPT"\naliases = ["ores-receipt"]\ntype = "string"\nhelp = "Optional matching ores.lambda.wasm-artifact.receipt/v1 build receipt. Requires --ores-adapter."\n'''
if flags.count(old) != 1:
    raise SystemExit("receipt flags: expected one adapter flag block")
flags_path.write_text(flags.replace(old, new, 1))
