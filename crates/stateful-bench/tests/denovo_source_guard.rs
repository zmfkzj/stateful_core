use std::{env, fs, path::Path, process::Command};

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = env::temp_dir().join(format!(
        "{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp root should be created");
    root
}

#[test]
fn stateful_off_omp_installs_non_stateful_source_guard_extension() {
    let root = temp_root("stateful-bench-denovo-source-guard");
    let adapter_script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/denovo_codex_agent.py");
    let output = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import importlib.util
import json
import pathlib
import sys

script = pathlib.Path(sys.argv[1])
root = pathlib.Path(sys.argv[2])
agent_dir = root / "agent"

spec = importlib.util.spec_from_file_location("denovo_codex_agent", script)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
sys.modules[spec.name] = module
spec.loader.exec_module(module)

env = {
    "HOME": str(root),
    "PI_CODING_AGENT_DIR": str(agent_dir),
    "STATEFUL_BENCHMARK_SOURCE_BLOCK_PATTERNS": json.dumps(["upstream"]),
}
module.prepare_omp_environment(
    env,
    enable_stateful=False,
    stateful_binary="/bin/false",
)
config = (agent_dir / "config.yml").read_text()
extension_path = agent_dir / "extensions" / "denovo-benchmark-source-guard.js"
extension = extension_path.read_text()
print(json.dumps({
    "config_has_extension": str(extension_path) in config,
    "extension_blocks_env": "STATEFUL_BENCHMARK_SOURCE_BLOCK_PATTERNS" in extension,
    "extension_uses_stateful": "runStatefulHook" in extension,
}))
"#,
        )
        .arg(&adapter_script)
        .arg(&root)
        .output()
        .expect("python should run source guard install check");

    let _ = fs::remove_dir_all(&root);
    assert!(
        output.status.success(),
        "source guard install check should run\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let decision: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("guard decision should be json");
    assert_eq!(
        decision["config_has_extension"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        decision["extension_blocks_env"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        decision["extension_uses_stateful"],
        serde_json::Value::Bool(false)
    );
}
