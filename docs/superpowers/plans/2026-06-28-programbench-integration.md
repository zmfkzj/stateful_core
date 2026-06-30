# ProgramBench Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add ProgramBench to `stateful-bench` as a Codex/OMP stateful-vs-no-state benchmark with official ProgramBench evaluation and efficiency reporting.

**Architecture:** Add a focused `programbench` Rust module beside `denovo`, plus two small Python adapter scripts for Codex and OMP. Rust owns CLI parsing, condition parsing, command construction, metadata/report schemas, official-eval orchestration, and comparison. Python owns CLI-specific agent invocation inside the ProgramBench container and emits per-instance metadata.

**Tech Stack:** Rust, clap, serde/serde_json, anyhow, std::process, Python 3 adapter scripts, Docker CLI, official `programbench` CLI.

## Global Constraints

- Preserve ProgramBench anti-cheat constraints: no internet/source lookup, no wrapping the provided binary, no decompilation, no `strace`/`ltrace` on the provided executable.
- Use official ProgramBench evaluation/scoring artifacts; do not fork or reinterpret official scoring.
- Default condition matrix is `stateful:off,subagent:off`, `stateful:on,subagent:off`, `stateful:off,subagent:on`, `stateful:on,subagent:on`.
- `programbench eval` runs `programbench submit package` by default after successful eval; `--no-package` may skip it for debug runs.
- Report quality and efficiency separately; no composite score may hide a quality regression behind lower cost.
- ProgramBench Docker images are Linux `amd64`; unit tests must not require Docker.
- No new Rust dependencies.
- Use TDD: write failing tests, run them red, implement the smallest change, run green, then commit.

---

## File Structure

- Create `crates/stateful-bench/src/programbench.rs`: ProgramBench CLI enum, condition parser, command builders, run/eval/report/compare orchestration, metadata structs, report rendering, unit-testable helpers.
- Modify `crates/stateful-bench/src/lib.rs`: expose `programbench`, re-export public ProgramBench interfaces, add `Command::Programbench`, dispatch to `programbench::run_programbench_cli`.
- Create `crates/stateful-bench/scripts/programbench_codex_agent.py`: Codex adapter invoked inside a prepared ProgramBench workspace, preserving ProgramBench prompt rules and emitting `instance.json` metadata.
- Create `crates/stateful-bench/scripts/programbench_omp_agent.py`: OMP adapter with the same metadata contract and OMP-specific invocation.
- Create `crates/stateful-bench/tests/programbench.rs`: CLI parser, condition parser, command builder, report aggregation, and compare tests.
- Modify `README.md`: mention ProgramBench support in Benchmark Tooling and Documentation.
- Modify `docs/usage-reference.md`: add `programbench` to Benchmark Commands.
- Create `docs/programbench-benchmark-guide.md`: setup, constraints, matrix, official scoring, efficiency reporting, and macOS/Linux `amd64` caveat.

---

### Task 1: CLI surface and condition parsing

**Files:**
- Create: `crates/stateful-bench/src/programbench.rs`
- Modify: `crates/stateful-bench/src/lib.rs:1-13`, `crates/stateful-bench/src/lib.rs:60-166`, `crates/stateful-bench/src/lib.rs:168-326`
- Test: `crates/stateful-bench/tests/programbench.rs`

**Interfaces:**
- Consumes: existing `ReportFormat` from `crate::ReportFormat`.
- Produces:
  - `ProgramBenchCommand`
  - `ProgramBenchAgentKind`
  - `ProgramBenchCondition`
  - `default_programbench_conditions() -> Vec<ProgramBenchCondition>`
  - `parse_programbench_condition(input: &str) -> anyhow::Result<ProgramBenchCondition>`
  - `run_programbench_cli(command: ProgramBenchCommand) -> anyhow::Result<()>`

- [ ] **Step 1: Write failing CLI and condition tests**

Create `crates/stateful-bench/tests/programbench.rs`:

```rust
use clap::Parser;
use stateful_bench::{
    Cli, Command, ProgramBenchAgentKind, ProgramBenchCommand, ProgramBenchCondition,
    ReportFormat, default_programbench_conditions, parse_programbench_condition,
};
use std::path::Path;

#[test]
fn programbench_run_command_parses_defaultable_options() {
    let cli = Cli::try_parse_from([
        "stateful-bench",
        "programbench",
        "run",
        "--output-dir",
        ".stateful_bench/programbench/runs",
        "--run-id",
        "pb-dev",
        "--agent",
        "codex-cli",
        "--condition",
        "stateful:on,subagent:off",
        "--model",
        "gpt-5.4-mini",
        "--benchmark-max-turns",
        "500",
        "--timeout-seconds",
        "7200",
        "--filter",
        "ripgrep.*",
        "--slice",
        "0:2",
        "--max-instances",
        "2",
        "--programbench-bin",
        "programbench",
        "--docker-bin",
        "docker",
        "--image-tag",
        "task_cleanroom_v6",
        "--stateful-binary",
        "target/debug/stateful",
    ])
    .expect("programbench run command should parse");

    assert!(matches!(
        cli.command,
        Command::Programbench {
            command: ProgramBenchCommand::Run {
                ref output_dir,
                ref run_id,
                agent: ProgramBenchAgentKind::CodexCli,
                ref condition,
                ref model,
                benchmark_max_turns: 500,
                timeout_seconds: 7200,
                ref filter,
                ref slice,
                max_instances: Some(2),
                ref programbench_bin,
                ref docker_bin,
                ref image_tag,
                ref stateful_binary,
                ..
            }
        } if output_dir == Path::new(".stateful_bench/programbench/runs")
            && run_id == "pb-dev"
            && condition == &vec!["stateful:on,subagent:off".to_string()]
            && model.as_deref() == Some("gpt-5.4-mini")
            && filter.as_deref() == Some("ripgrep.*")
            && slice.as_deref() == Some("0:2")
            && programbench_bin == "programbench"
            && docker_bin == "docker"
            && image_tag == "task_cleanroom_v6"
            && stateful_binary == "target/debug/stateful"
    ));
}

#[test]
fn programbench_eval_report_compare_commands_parse() {
    let eval = Cli::try_parse_from([
        "stateful-bench",
        "programbench",
        "eval",
        "--run-dir",
        ".stateful_bench/programbench/runs/pb-dev",
        "--workers",
        "4",
        "--branch-workers",
        "2",
        "--docker-cpus",
        "8",
        "--force",
        "--no-package",
    ])
    .expect("programbench eval should parse");
    assert!(matches!(
        eval.command,
        Command::Programbench {
            command: ProgramBenchCommand::Eval {
                ref run_dir,
                workers: 4,
                branch_workers: 2,
                docker_cpus: 8,
                force: true,
                no_package: true,
                ..
            }
        } if run_dir == Path::new(".stateful_bench/programbench/runs/pb-dev")
    ));

    let report = Cli::try_parse_from([
        "stateful-bench",
        "programbench",
        "report",
        "--condition-dir",
        "runs/pb-dev/conditions/stateful-on_subagent-off",
        "--format",
        "markdown",
    ])
    .expect("programbench report should parse");
    assert!(matches!(
        report.command,
        Command::Programbench {
            command: ProgramBenchCommand::Report {
                ref condition_dir,
                format: ReportFormat::Markdown,
                ..
            }
        } if condition_dir == Path::new("runs/pb-dev/conditions/stateful-on_subagent-off")
    ));

    let compare = Cli::try_parse_from([
        "stateful-bench",
        "programbench",
        "compare",
        "--report",
        "stateful-off.json",
        "--report",
        "stateful-on.json",
    ])
    .expect("programbench compare should parse");
    assert!(matches!(
        compare.command,
        Command::Programbench {
            command: ProgramBenchCommand::Compare { ref report, .. }
        } if report == &vec!["stateful-off.json".into(), "stateful-on.json".into()]
    ));
}

#[test]
fn programbench_condition_parser_accepts_axes_and_defaults_cover_four_conditions() {
    let condition = parse_programbench_condition("stateful:on,subagent:off")
        .expect("condition should parse");

    assert!(condition.stateful);
    assert!(!condition.subagent);
    assert_eq!(condition.id(), "stateful-on_subagent-off");

    assert_eq!(
        default_programbench_conditions()
            .iter()
            .map(ProgramBenchCondition::id)
            .collect::<Vec<_>>(),
        vec![
            "stateful-off_subagent-off",
            "stateful-on_subagent-off",
            "stateful-off_subagent-on",
            "stateful-on_subagent-on",
        ]
    );
}

#[test]
fn programbench_condition_parser_rejects_unknown_keys() {
    let error = parse_programbench_condition("stateful:on,unknown:off")
        .expect_err("unknown condition key should fail");

    assert!(
        error
            .to_string()
            .contains("unknown ProgramBench condition key `unknown`"),
        "unexpected error: {error}"
    );
}
```

- [ ] **Step 2: Run tests red**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_run_command_parses_defaultable_options programbench_eval_report_compare_commands_parse programbench_condition_parser_accepts_axes_and_defaults_cover_four_conditions programbench_condition_parser_rejects_unknown_keys
```

Expected: FAIL because `programbench` module, command variants, and condition parser do not exist.

- [ ] **Step 3: Add minimal ProgramBench module and CLI dispatch**

Create `crates/stateful-bench/src/programbench.rs`:

```rust
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::ReportFormat;

const DEFAULT_PROGRAMBENCH_RUNS: &str = ".stateful_bench/programbench/runs";
const DEFAULT_PROGRAMBENCH_IMAGE_TAG: &str = "task_cleanroom_v6";
const DEFAULT_PROGRAMBENCH_BIN: &str = "programbench";
const DEFAULT_DOCKER_BIN: &str = "docker";
const DEFAULT_CODEX_BIN: &str = "codex";
const DEFAULT_OMP_BIN: &str = "omp";
const DEFAULT_BENCHMARK_MAX_TURNS: usize = 500;
const DEFAULT_TIMEOUT_SECONDS: u64 = 7200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum ProgramBenchAgentKind {
    CodexCli,
    OmpCli,
}

#[derive(Debug, Subcommand)]
pub enum ProgramBenchCommand {
    Run {
        #[arg(long, default_value = DEFAULT_PROGRAMBENCH_RUNS)]
        output_dir: PathBuf,
        #[arg(long, default_value_t = default_programbench_run_id())]
        run_id: String,
        #[arg(long, value_enum, default_value_t = ProgramBenchAgentKind::CodexCli)]
        agent: ProgramBenchAgentKind,
        #[arg(long)]
        condition: Vec<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value_t = DEFAULT_BENCHMARK_MAX_TURNS)]
        benchmark_max_turns: usize,
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long)]
        filter: Option<String>,
        #[arg(long)]
        slice: Option<String>,
        #[arg(long)]
        max_instances: Option<usize>,
        #[arg(long, default_value = DEFAULT_PROGRAMBENCH_BIN)]
        programbench_bin: String,
        #[arg(long, default_value = DEFAULT_DOCKER_BIN)]
        docker_bin: String,
        #[arg(long, default_value = DEFAULT_PROGRAMBENCH_IMAGE_TAG)]
        image_tag: String,
        #[arg(long, default_value = "stateful")]
        stateful_binary: String,
        #[arg(long, default_value = DEFAULT_CODEX_BIN)]
        codex_bin: String,
        #[arg(long, default_value = DEFAULT_OMP_BIN)]
        omp_bin: String,
    },
    Eval {
        #[arg(long)]
        run_dir: PathBuf,
        #[arg(long, default_value = DEFAULT_PROGRAMBENCH_BIN)]
        programbench_bin: String,
        #[arg(long, default_value_t = 1)]
        workers: usize,
        #[arg(long, default_value_t = 1)]
        branch_workers: usize,
        #[arg(long, default_value_t = 8)]
        docker_cpus: usize,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        no_package: bool,
    },
    Report {
        #[arg(long)]
        condition_dir: PathBuf,
        #[arg(long, value_enum, default_value_t = ReportFormat::Json)]
        format: ReportFormat,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Compare {
        #[arg(long, required = true)]
        report: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = ReportFormat::Json)]
        format: ReportFormat,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn default_programbench_run_id() -> String {
    format!("programbench-{}", uuid::Uuid::new_v4())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramBenchCondition {
    pub stateful: bool,
    pub subagent: bool,
}

impl ProgramBenchCondition {
    pub fn new(stateful: bool, subagent: bool) -> Self {
        Self { stateful, subagent }
    }

    pub fn id(self) -> String {
        format!(
            "stateful-{}_subagent-{}",
            axis_label(self.stateful),
            axis_label(self.subagent)
        )
    }
}

pub fn default_programbench_conditions() -> Vec<ProgramBenchCondition> {
    vec![
        ProgramBenchCondition::new(false, false),
        ProgramBenchCondition::new(true, false),
        ProgramBenchCondition::new(false, true),
        ProgramBenchCondition::new(true, true),
    ]
}

pub fn parse_programbench_condition(input: &str) -> Result<ProgramBenchCondition> {
    let mut stateful = None;
    let mut subagent = None;
    for raw_part in input.split(',') {
        let (key, value) = raw_part
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid ProgramBench condition axis `{raw_part}`"))?;
        match key.trim() {
            "stateful" => stateful = Some(parse_axis(value.trim())?),
            "subagent" => subagent = Some(parse_axis(value.trim())?),
            unknown => bail!("unknown ProgramBench condition key `{unknown}`"),
        }
    }
    Ok(ProgramBenchCondition::new(
        stateful.unwrap_or(false),
        subagent.unwrap_or(false),
    ))
}

pub fn run_programbench_cli(_command: ProgramBenchCommand) -> Result<()> {
    bail!("ProgramBench command execution is not implemented yet")
}

fn parse_axis(value: &str) -> Result<bool> {
    match value {
        "on" | "true" => Ok(true),
        "off" | "false" => Ok(false),
        _ => bail!("expected axis value `on` or `off`, got `{value}`"),
    }
}

fn axis_label(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}
```

Modify `crates/stateful-bench/src/lib.rs`:

```rust
pub mod denovo;
pub mod programbench;

pub use programbench::{
    ProgramBenchAgentKind, ProgramBenchCommand, ProgramBenchCondition,
    default_programbench_conditions, parse_programbench_condition,
};
```

Add the command variant:

```rust
    Programbench {
        #[command(subcommand)]
        command: programbench::ProgramBenchCommand,
    },
```

Add run dispatch:

```rust
        Command::Programbench { command } => {
            programbench::run_programbench_cli(command)?;
        }
```

- [ ] **Step 4: Run tests green**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_run_command_parses_defaultable_options programbench_eval_report_compare_commands_parse programbench_condition_parser_accepts_axes_and_defaults_cover_four_conditions programbench_condition_parser_rejects_unknown_keys
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-bench/src/lib.rs crates/stateful-bench/src/programbench.rs crates/stateful-bench/tests/programbench.rs
git commit -m "Add ProgramBench CLI skeleton"
```

---

### Task 2: Command builders and metadata schema

**Files:**
- Modify: `crates/stateful-bench/src/programbench.rs`
- Modify: `crates/stateful-bench/src/lib.rs`
- Modify: `crates/stateful-bench/tests/programbench.rs`

**Interfaces:**
- Consumes: `ProgramBenchAgentKind`, `ProgramBenchCondition`.
- Produces:
  - `ProgramBenchRecipeCommand { program: String, args: Vec<String>, env: BTreeMap<String, String> }`
  - `ProgramBenchInstanceRunOptions`
  - `ProgramBenchRunOptions`
  - `ProgramBenchTokenUsage`
  - `ProgramBenchInstanceMetadata`
  - `ProgramBenchConditionMetadata`
  - `build_programbench_agent_command(options: ProgramBenchInstanceRunOptions) -> anyhow::Result<ProgramBenchRecipeCommand>`

- [ ] **Step 1: Write failing command-builder tests**

Append to `crates/stateful-bench/tests/programbench.rs`:

```rust
use stateful_bench::{
    ProgramBenchInstanceRunOptions, build_programbench_agent_command,
};

#[test]
fn programbench_codex_agent_command_contains_condition_and_paths() {
    let command = build_programbench_agent_command(ProgramBenchInstanceRunOptions {
        agent: ProgramBenchAgentKind::CodexCli,
        condition: ProgramBenchCondition::new(true, false),
        instance_id: "BurntSushi__ripgrep.abc123".to_string(),
        container_id: "programbench-container".to_string(),
        condition_dir: "runs/pb/conditions/stateful-on_subagent-off".into(),
        docker_bin: "docker".to_string(),
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "target/debug/stateful".to_string(),
        model: Some("gpt-5.4-mini".to_string()),
        benchmark_max_turns: 500,
        timeout_seconds: 7200,
        subagent_min_count: 3,
    })
    .expect("command should build");

    assert!(command.program.ends_with("programbench_codex_agent.py"));
    assert!(command.args.contains(&"--container-id".to_string()));
    assert!(command.args.contains(&"programbench-container".to_string()));
    assert!(command.args.contains(&"--condition-id".to_string()));
    assert!(command.args.contains(&"stateful-on_subagent-off".to_string()));
    assert!(command.args.contains(&"--stateful".to_string()));
    assert!(!command.args.contains(&"--subagent".to_string()));
}

#[test]
fn programbench_omp_agent_command_marks_subagent_condition() {
    let command = build_programbench_agent_command(ProgramBenchInstanceRunOptions {
        agent: ProgramBenchAgentKind::OmpCli,
        condition: ProgramBenchCondition::new(false, true),
        instance_id: "ajeetdsouza__zoxide.67ca1bc".to_string(),
        container_id: "programbench-container".to_string(),
        condition_dir: "runs/pb/conditions/stateful-off_subagent-on".into(),
        docker_bin: "docker".to_string(),
        codex_bin: "codex".to_string(),
        omp_bin: "omp".to_string(),
        stateful_binary: "stateful".to_string(),
        model: None,
        benchmark_max_turns: 500,
        timeout_seconds: 7200,
        subagent_min_count: 3,
    })
    .expect("command should build");

    assert!(command.program.ends_with("programbench_omp_agent.py"));
    assert!(command.args.contains(&"--subagent".to_string()));
    assert!(!command.args.contains(&"--stateful".to_string()));
}
```

- [ ] **Step 2: Run tests red**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_codex_agent_command_contains_condition_and_paths programbench_omp_agent_command_marks_subagent_condition
```

Expected: FAIL because command-builder types and functions do not exist.

- [ ] **Step 3: Implement command builders and metadata**

Add to `crates/stateful-bench/src/programbench.rs`:

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramBenchRecipeCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramBenchInstanceRunOptions {
    pub agent: ProgramBenchAgentKind,
    pub condition: ProgramBenchCondition,
    pub instance_id: String,
    pub container_id: String,
    pub condition_dir: PathBuf,
    pub docker_bin: String,
    pub codex_bin: String,
    pub omp_bin: String,
    pub stateful_binary: String,
    pub model: Option<String>,
    pub benchmark_max_turns: usize,
    pub timeout_seconds: u64,
    pub subagent_min_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramBenchRunOptions {
    pub output_dir: PathBuf,
    pub run_id: String,
    pub agent: ProgramBenchAgentKind,
    pub conditions: Vec<ProgramBenchCondition>,
    pub model: Option<String>,
    pub benchmark_max_turns: usize,
    pub timeout_seconds: u64,
    pub filter: Option<String>,
    pub slice: Option<String>,
    pub max_instances: Option<usize>,
    pub programbench_bin: String,
    pub docker_bin: String,
    pub image_tag: String,
    pub stateful_binary: String,
    pub codex_bin: String,
    pub omp_bin: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramBenchTokenUsage {
    #[serde(default)]
    pub turns: usize,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_output_tokens: u64,
    #[serde(default)]
    pub input_plus_output_tokens: u64,
    #[serde(default)]
    pub uncached_input_tokens: u64,
    #[serde(default)]
    pub uncached_input_plus_output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramBenchInstanceMetadata {
    pub instance_id: String,
    pub condition_id: String,
    pub agent: ProgramBenchAgentKind,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub running_time_ms: u64,
    pub submission_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_used: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<ProgramBenchTokenUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramBenchConditionMetadata {
    pub run_id: String,
    pub condition_id: String,
    pub condition: ProgramBenchCondition,
    pub agent: ProgramBenchAgentKind,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub running_time_ms: u64,
    pub instances: Vec<ProgramBenchInstanceMetadata>,
}

pub fn build_programbench_agent_command(
    options: ProgramBenchInstanceRunOptions,
) -> Result<ProgramBenchRecipeCommand> {
    let script = default_programbench_agent_script(options.agent);
    let mut args = vec![
        "--container-id".to_string(),
        options.container_id,
        "--instance-id".to_string(),
        options.instance_id,
        "--condition-id".to_string(),
        options.condition.id(),
        "--condition-dir".to_string(),
        path_arg(&options.condition_dir),
        "--docker-bin".to_string(),
        options.docker_bin,
        "--stateful-binary".to_string(),
        options.stateful_binary,
        "--benchmark-max-turns".to_string(),
        options.benchmark_max_turns.to_string(),
        "--timeout-seconds".to_string(),
        options.timeout_seconds.to_string(),
        "--subagent-min-count".to_string(),
        options.subagent_min_count.to_string(),
    ];
    match options.agent {
        ProgramBenchAgentKind::CodexCli => args.extend(["--codex-bin".to_string(), options.codex_bin]),
        ProgramBenchAgentKind::OmpCli => args.extend(["--omp-bin".to_string(), options.omp_bin]),
    }
    if options.condition.stateful {
        args.push("--stateful".to_string());
    }
    if options.condition.subagent {
        args.push("--subagent".to_string());
    }
    if let Some(model) = options.model {
        args.extend(["--model".to_string(), model]);
    }
    Ok(ProgramBenchRecipeCommand {
        program: path_arg(&script),
        args,
        env: BTreeMap::new(),
    })
}

fn default_programbench_agent_script(agent: ProgramBenchAgentKind) -> PathBuf {
    let name = match agent {
        ProgramBenchAgentKind::CodexCli => "programbench_codex_agent.py",
        ProgramBenchAgentKind::OmpCli => "programbench_omp_agent.py",
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts").join(name)
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
```

Re-export all new public types/functions from `lib.rs`.

- [ ] **Step 4: Run tests green**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_codex_agent_command_contains_condition_and_paths programbench_omp_agent_command_marks_subagent_condition
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-bench/src/lib.rs crates/stateful-bench/src/programbench.rs crates/stateful-bench/tests/programbench.rs
git commit -m "Add ProgramBench command builders"
```

---

### Task 3: Official score and efficiency reports

**Files:**
- Modify: `crates/stateful-bench/src/programbench.rs`
- Modify: `crates/stateful-bench/src/lib.rs`
- Modify: `crates/stateful-bench/tests/programbench.rs`

**Interfaces:**
- Consumes: `ProgramBenchConditionMetadata`, `ProgramBenchInstanceMetadata`, `ProgramBenchTokenUsage`, `_stats/score.json`.
- Produces:
  - `ProgramBenchConditionReport`
  - `ProgramBenchComparisonReport`
  - `build_programbench_condition_report(condition_dir: impl AsRef<Path>) -> anyhow::Result<ProgramBenchConditionReport>`
  - `compare_programbench_reports(reports: Vec<ProgramBenchConditionReport>) -> ProgramBenchComparisonReport`
  - `render_programbench_report_markdown(report: &ProgramBenchConditionReport) -> String`
  - `render_programbench_comparison_markdown(report: &ProgramBenchComparisonReport) -> String`

- [ ] **Step 1: Write failing report aggregation test**

Append to `crates/stateful-bench/tests/programbench.rs`:

```rust
use stateful_bench::{
    ProgramBenchConditionMetadata, ProgramBenchConditionReport, ProgramBenchInstanceMetadata,
    ProgramBenchTokenUsage, build_programbench_condition_report, compare_programbench_reports,
};
use std::fs;

#[test]
fn programbench_report_aggregates_official_score_and_efficiency() {
    let root = temp_root("stateful-bench-programbench-report");
    let condition_dir = root.join("conditions/stateful-on_subagent-on");
    fs::create_dir_all(condition_dir.join("instance-a")).expect("instance dir should exist");
    fs::create_dir_all(condition_dir.join("instance-b")).expect("instance dir should exist");
    fs::create_dir_all(condition_dir.join("_stats")).expect("stats dir should exist");

    let metadata = ProgramBenchConditionMetadata {
        run_id: "pb-dev".to_string(),
        condition_id: "stateful-on_subagent-on".to_string(),
        condition: ProgramBenchCondition::new(true, true),
        agent: ProgramBenchAgentKind::CodexCli,
        started_at_ms: 10,
        finished_at_ms: 4010,
        running_time_ms: 4000,
        instances: vec![
            instance_metadata("instance-a", None, Some(true), token_usage(2, 100, 40, 12, 5)),
            instance_metadata("instance-b", Some("agent exited 1"), Some(false), token_usage(1, 50, 20, 5, 2)),
        ],
    };
    fs::write(
        condition_dir.join("condition.json"),
        serde_json::to_string_pretty(&metadata).expect("metadata should serialize"),
    )
    .expect("condition metadata should write");
    fs::write(
        condition_dir.join("_stats/score.json"),
        r#"{
          "instance-a": {"test-a": true, "test-b": true},
          "instance-b": {"test-c": true, "test-d": false}
        }"#,
    )
    .expect("score should write");

    let report = build_programbench_condition_report(&condition_dir)
        .expect("report should build");

    assert_eq!(report.condition_id, "stateful-on_subagent-on");
    assert_eq!(report.instances, 2);
    assert_eq!(report.evaluated_instances, 2);
    assert_eq!(report.agent_error_count, 1);
    assert_eq!(report.average_score, Some(0.75));
    assert_eq!(report.resolved_count, 1);
    assert_eq!(report.resolved_rate, Some(0.5));
    assert_eq!(report.running_time_ms, 4000);
    assert_eq!(report.average_running_time_ms, Some(2000.0));
    assert_eq!(report.token_observed_instances, 2);
    assert_eq!(report.token_usage_turns, 3);
    assert_eq!(report.token_input_plus_output_tokens, 167);
    assert_eq!(report.token_uncached_input_plus_output_tokens, 107);
    assert_eq!(report.average_input_plus_output_tokens, Some(83.5));
    assert_eq!(report.average_uncached_input_plus_output_tokens, Some(53.5));
    assert_eq!(report.subagent_observed_instances, 2);
    assert_eq!(report.subagent_used_count, 1);
    assert_eq!(report.subagent_used_rate, Some(0.5));
    assert_eq!(report.score_source, "score-json");

    fs::remove_dir_all(root).expect("temp root should clean up");
}

fn instance_metadata(
    instance_id: &str,
    error: Option<&str>,
    subagent_used: Option<bool>,
    token_usage: ProgramBenchTokenUsage,
) -> ProgramBenchInstanceMetadata {
    ProgramBenchInstanceMetadata {
        instance_id: instance_id.to_string(),
        condition_id: "stateful-on_subagent-on".to_string(),
        agent: ProgramBenchAgentKind::CodexCli,
        started_at_ms: 10,
        finished_at_ms: 1010,
        running_time_ms: 1000,
        submission_path: format!("{instance_id}/submission.tar.gz"),
        exit_code: Some(if error.is_some() { 1 } else { 0 }),
        error: error.map(str::to_string),
        subagent_used,
        token_usage: Some(token_usage),
    }
}

fn token_usage(
    turns: usize,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
) -> ProgramBenchTokenUsage {
    ProgramBenchTokenUsage {
        turns,
        input_tokens,
        cached_input_tokens,
        output_tokens,
        reasoning_output_tokens,
        input_plus_output_tokens: input_tokens + output_tokens,
        uncached_input_tokens: input_tokens.saturating_sub(cached_input_tokens),
        uncached_input_plus_output_tokens: input_tokens.saturating_sub(cached_input_tokens) + output_tokens,
    }
}
```

- [ ] **Step 2: Write failing comparison test**

Append:

```rust
#[test]
fn programbench_compare_reports_score_time_and_token_deltas() {
    let off = condition_report("stateful-off_subagent-off", false, false, 0.5, 6000, 220, 140);
    let on = condition_report("stateful-on_subagent-off", true, false, 0.75, 4000, 167, 107);

    let comparison = compare_programbench_reports(vec![off, on]);

    assert_eq!(comparison.stateful_score_delta_without_subagent, Some(0.25));
    assert_eq!(comparison.stateful_running_time_ms_delta_without_subagent, Some(-2000));
    assert_eq!(comparison.stateful_input_plus_output_tokens_delta_without_subagent, Some(-53));
    assert_eq!(comparison.missing_axis_ids, vec![
        "stateful-off_subagent-on".to_string(),
        "stateful-on_subagent-on".to_string(),
    ]);
}

fn condition_report(
    condition_id: &str,
    stateful: bool,
    subagent: bool,
    score: f64,
    running_time_ms: u64,
    input_plus_output: u64,
    uncached_input_plus_output: u64,
) -> ProgramBenchConditionReport {
    ProgramBenchConditionReport {
        run_id: "pb-dev".to_string(),
        condition_id: condition_id.to_string(),
        condition: ProgramBenchCondition::new(stateful, subagent),
        instances: 2,
        attempted_instances: 2,
        evaluated_instances: 2,
        average_score: Some(score),
        resolved_count: 0,
        resolved_rate: Some(0.0),
        eval_error_count: 0,
        agent_error_count: 0,
        timeout_count: 0,
        running_time_ms,
        average_running_time_ms: Some(running_time_ms as f64 / 2.0),
        token_observed_instances: 2,
        token_usage_turns: 4,
        token_input_tokens: 0,
        token_cached_input_tokens: 0,
        token_output_tokens: 0,
        token_reasoning_output_tokens: 0,
        token_input_plus_output_tokens: input_plus_output,
        token_uncached_input_tokens: 0,
        token_uncached_input_plus_output_tokens: uncached_input_plus_output,
        average_input_plus_output_tokens: Some(input_plus_output as f64 / 2.0),
        average_uncached_input_plus_output_tokens: Some(uncached_input_plus_output as f64 / 2.0),
        subagent_observed_instances: 0,
        subagent_used_count: 0,
        subagent_used_rate: None,
        score_per_million_input_plus_output_tokens: Some(score * 1_000_000.0 / input_plus_output as f64),
        score_per_million_uncached_input_plus_output_tokens: Some(score * 1_000_000.0 / uncached_input_plus_output as f64),
        score_per_hour: Some(score * 3_600_000.0 / running_time_ms as f64),
        score_source: "score-json".to_string(),
    }
}
```

- [ ] **Step 3: Run tests red**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_report_aggregates_official_score_and_efficiency programbench_compare_reports_score_time_and_token_deltas
```

Expected: FAIL because report types and functions do not exist.

- [ ] **Step 4: Implement reports**

Add `ProgramBenchConditionReport`, `ProgramBenchComparisonReport`, `build_programbench_condition_report`, `compare_programbench_reports`, markdown renderers, JSON/Markdown `render` methods, and private helpers mirroring the DeNovo `ratio`, `average`, `average_u64`, `round_three`, `delta`, and `score_for` pattern. Read `_stats/score.json` as `BTreeMap<String, BTreeMap<String, bool>>`; each instance score is `passed_tests / total_tests` and full resolution is score `>= 1.0`.

Implement token fallback methods exactly:

```rust
impl ProgramBenchTokenUsage {
    fn has_observed_tokens(&self) -> bool {
        self.turns > 0
            || self.input_tokens > 0
            || self.cached_input_tokens > 0
            || self.output_tokens > 0
            || self.reasoning_output_tokens > 0
    }

    fn input_plus_output_tokens(&self) -> u64 {
        if self.input_plus_output_tokens == 0 {
            self.input_tokens + self.output_tokens
        } else {
            self.input_plus_output_tokens
        }
    }

    fn uncached_input_tokens(&self) -> u64 {
        if self.uncached_input_tokens == 0 {
            self.input_tokens.saturating_sub(self.cached_input_tokens)
        } else {
            self.uncached_input_tokens
        }
    }

    fn uncached_input_plus_output_tokens(&self) -> u64 {
        if self.uncached_input_plus_output_tokens == 0 {
            self.uncached_input_tokens() + self.output_tokens
        } else {
            self.uncached_input_plus_output_tokens
        }
    }
}
```

Re-export report interfaces from `lib.rs`.

- [ ] **Step 5: Run tests green**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_report_aggregates_official_score_and_efficiency programbench_compare_reports_score_time_and_token_deltas
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/stateful-bench/src/lib.rs crates/stateful-bench/src/programbench.rs crates/stateful-bench/tests/programbench.rs
git commit -m "Add ProgramBench reporting metrics"
```

---

### Task 4: Adapter scripts and token extraction

**Files:**
- Create: `crates/stateful-bench/scripts/programbench_codex_agent.py`
- Create: `crates/stateful-bench/scripts/programbench_omp_agent.py`
- Modify: `crates/stateful-bench/tests/programbench.rs`

**Interfaces:**
- Consumes: command arguments emitted by `build_programbench_agent_command`.
- Produces: `<condition-dir>/<instance_id>/instance.json`, `agent.stdout.log`, `agent.stderr.log`, and `submission.tar.gz`.

- [ ] **Step 1: Write failing Python adapter tests from Rust**

Append tests that import each script and exercise token parsing without launching Codex, OMP, or Docker:

```rust
use std::process::Command as ProcessCommand;

#[test]
fn programbench_codex_adapter_parses_token_usage_events() {
    let script = format!(
        r#"
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location("programbench_codex_agent", {agent_path:?})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)
usage = mod.codex_token_usage_from_output('{{"type":"turn.completed","usage":{{"input_tokens":100,"cached_input_tokens":40,"output_tokens":12,"reasoning_output_tokens":5}}}}\n')
print(json.dumps(usage, sort_keys=True))
"#,
        agent_path = programbench_codex_agent_path().to_string_lossy(),
    );
    let output = ProcessCommand::new("python3").arg("-c").arg(script).output().expect("python should run");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json should parse");
    assert_eq!(value["input_tokens"], 100);
    assert_eq!(value["cached_input_tokens"], 40);
    assert_eq!(value["output_tokens"], 12);
    assert_eq!(value["input_plus_output_tokens"], 112);
    assert_eq!(value["uncached_input_plus_output_tokens"], 72);
}

#[test]
fn programbench_omp_adapter_parses_token_usage_events() {
    let script = format!(
        r#"
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location("programbench_omp_agent", {agent_path:?})
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)
usage = mod.omp_token_usage_from_output('{{"usage":{{"input_tokens":50,"cached_input_tokens":20,"output_tokens":5,"reasoning_output_tokens":2}}}}\n')
print(json.dumps(usage, sort_keys=True))
"#,
        agent_path = programbench_omp_agent_path().to_string_lossy(),
    );
    let output = ProcessCommand::new("python3").arg("-c").arg(script).output().expect("python should run");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json should parse");
    assert_eq!(value["input_tokens"], 50);
    assert_eq!(value["input_plus_output_tokens"], 55);
    assert_eq!(value["uncached_input_plus_output_tokens"], 35);
}

fn programbench_codex_agent_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/programbench_codex_agent.py")
}

fn programbench_omp_agent_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/programbench_omp_agent.py")
}
```

- [ ] **Step 2: Run tests red**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_codex_adapter_parses_token_usage_events programbench_omp_adapter_parses_token_usage_events
```

Expected: FAIL because adapter scripts do not exist.

- [ ] **Step 3: Add adapter scripts**

Create `crates/stateful-bench/scripts/programbench_codex_agent.py` with:

```python
#!/usr/bin/env python3
"""Run one ProgramBench instance with Codex CLI and write stateful-bench metadata."""

from __future__ import annotations

import argparse
import json
import subprocess
import time
from pathlib import Path
from typing import Any

PROGRAMBENCH_SYSTEM_PROMPT = """You are solving one ProgramBench instance.

You are given a compiled ./executable and bundled documentation. Rebuild an original source codebase from scratch so ./compile.sh creates a replacement ./executable with matching behavior.

Rules:
- Do not search the internet, clone repositories, or fetch package/source registry copies of the target project.
- Do not wrap, copy, chmod, or delegate to the provided ./executable.
- Do not decompile ./executable or use strace/ltrace on it.
- You may run ./executable normally and read bundled documentation.
- Leave a working ./compile.sh that builds ./executable.
""".strip()


def empty_token_usage() -> dict[str, int]:
    return {"turns": 0, "input_tokens": 0, "cached_input_tokens": 0, "output_tokens": 0, "reasoning_output_tokens": 0, "input_plus_output_tokens": 0, "uncached_input_tokens": 0, "uncached_input_plus_output_tokens": 0}


def iter_json_events(output: str):
    for line in output.splitlines():
        try:
            yield json.loads(line)
        except json.JSONDecodeError:
            continue


def token_usage_from_value(value: Any) -> dict[str, int] | None:
    if not isinstance(value, dict):
        return None
    input_tokens = int(value.get("input_tokens") or 0)
    cached_input_tokens = int(value.get("cached_input_tokens") or value.get("input_tokens_details", {}).get("cached_tokens") or 0)
    output_tokens = int(value.get("output_tokens") or 0)
    reasoning_output_tokens = int(value.get("reasoning_output_tokens") or value.get("output_tokens_details", {}).get("reasoning_tokens") or 0)
    total = int(value.get("total_tokens") or value.get("token_count") or input_tokens + output_tokens)
    if not any([total, input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens]):
        return None
    uncached = max(input_tokens - cached_input_tokens, 0)
    return {"turns": 1, "input_tokens": input_tokens, "cached_input_tokens": cached_input_tokens, "output_tokens": output_tokens, "reasoning_output_tokens": reasoning_output_tokens, "input_plus_output_tokens": input_tokens + output_tokens, "uncached_input_tokens": uncached, "uncached_input_plus_output_tokens": uncached + output_tokens}


def codex_token_usage_from_output(output: str) -> dict[str, int]:
    total = empty_token_usage()
    for event in iter_json_events(output):
        if not isinstance(event, dict):
            continue
        candidates = [event.get("usage"), event.get("info", {}).get("total_token_usage"), event.get("payload", {}).get("usage"), event.get("payload", {}).get("info", {}).get("total_token_usage")]
        for candidate in candidates:
            usage = token_usage_from_value(candidate)
            if usage:
                for key in total:
                    total[key] += usage[key]
                break
    return total


def archive_workspace(args: argparse.Namespace, instance_dir: Path) -> Path:
    archive = instance_dir / "submission.tar.gz"
    subprocess.run([args.docker_bin, "exec", args.container_id, "tar", "-C", "/workspace", "-czf", "/tmp/submission.tar.gz", "."], check=True, timeout=300)
    subprocess.run([args.docker_bin, "cp", f"{args.container_id}:/tmp/submission.tar.gz", str(archive)], check=True, timeout=300)
    return archive


def run_agent(args: argparse.Namespace, prompt: str) -> subprocess.CompletedProcess[str]:
    command = [args.codex_bin, "exec", "--json", "--cd", "/workspace"]
    if args.model:
        command.extend(["--model", args.model])
    command.append(prompt)
    return subprocess.run(command, text=True, capture_output=True, timeout=args.timeout_seconds)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--container-id", required=True)
    parser.add_argument("--instance-id", required=True)
    parser.add_argument("--condition-id", required=True)
    parser.add_argument("--condition-dir", required=True)
    parser.add_argument("--docker-bin", default="docker")
    parser.add_argument("--codex-bin", default="codex")
    parser.add_argument("--stateful-binary", default="stateful")
    parser.add_argument("--model")
    parser.add_argument("--benchmark-max-turns", type=int, default=500)
    parser.add_argument("--timeout-seconds", type=int, default=7200)
    parser.add_argument("--subagent-min-count", type=int, default=3)
    parser.add_argument("--stateful", action="store_true")
    parser.add_argument("--subagent", action="store_true")
    args = parser.parse_args()
    started = int(time.time() * 1000)
    instance_dir = Path(args.condition_dir) / args.instance_id
    instance_dir.mkdir(parents=True, exist_ok=True)
    prompt = PROGRAMBENCH_SYSTEM_PROMPT
    if args.subagent:
        prompt += f"\n\nUse at least {args.subagent_min_count} native subagents before implementation."
    completed = run_agent(args, prompt)
    (instance_dir / "agent.stdout.log").write_text(completed.stdout)
    (instance_dir / "agent.stderr.log").write_text(completed.stderr)
    archive = archive_workspace(args, instance_dir)
    finished = int(time.time() * 1000)
    metadata = {"instance_id": args.instance_id, "condition_id": args.condition_id, "agent": "codex-cli", "started_at_ms": started, "finished_at_ms": finished, "running_time_ms": finished - started, "submission_path": str(archive), "exit_code": completed.returncode, "error": None if completed.returncode == 0 else f"codex exited {completed.returncode}", "subagent_used": args.subagent, "token_usage": codex_token_usage_from_output(completed.stdout)}
    (instance_dir / "instance.json").write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n")
    return completed.returncode


if __name__ == "__main__":
    raise SystemExit(main())
```

Create `crates/stateful-bench/scripts/programbench_omp_agent.py` with:

```python
#!/usr/bin/env python3
"""Run one ProgramBench instance with OMP CLI and write stateful-bench metadata."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from programbench_codex_agent import PROGRAMBENCH_SYSTEM_PROMPT, archive_workspace, empty_token_usage, output_text, token_usage_from_value


def iter_json_events(output: str):
    for line in output.splitlines():
        try:
            yield json.loads(line)
        except json.JSONDecodeError:
            continue


def omp_token_usage_from_output(output: str) -> dict[str, int]:
    total = empty_token_usage()
    for event in iter_json_events(output):
        if not isinstance(event, dict):
            continue
        for candidate in [event.get("usage"), event.get("payload", {}).get("usage")]:
            usage = token_usage_from_value(candidate)
            if usage:
                for key in total:
                    total[key] += usage[key]
                break
    return total
```

Then copy the Codex `main()` structure, replacing `--codex-bin` with `--omp-bin`, `agent` with `omp-cli`, `codex exited` with `omp exited`, token parser with `omp_token_usage_from_output`, and using a dedicated OMP runner that preserves timeout metadata. Invoke OMP with `--cwd` set to the temporary host airlock rather than `/workspace` inside the container. For `stateful:on` OMP runs, copy `STATEFUL_SERVER_URL` and `STATEFUL_SERVER_TOKEN` from the parent environment into the agent environment when both are present. The copied `main()` must preserve `subprocess.TimeoutExpired` as the primary result: exit code `124`, error text `omp timed out after {args.timeout_seconds}s`, captured partial stdout/stderr logs, and optional `cleanup_error` metadata if killing or draining the timed-out process fails.

```python
def run_omp_command(command, *, cwd: str, env: dict[str, str], timeout_seconds: int):
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=cwd, env=env, text=True)
    try:
        stdout, stderr = process.communicate(timeout=timeout_seconds)
        return subprocess.CompletedProcess(command, process.returncode, stdout=stdout, stderr=stderr)
    except subprocess.TimeoutExpired as exc:
        cleanup_error = None
        try:
            process.kill()
        except Exception as kill_exc:
            cleanup_error = str(kill_exc)
        else:
            try:
                stdout, stderr = process.communicate()
                exc.output = output_text(stdout) or output_text(exc.output)
                exc.stderr = output_text(stderr) or output_text(exc.stderr)
            except Exception as wait_exc:
                cleanup_error = str(wait_exc)
        if cleanup_error is not None:
            exc.cleanup_error = cleanup_error
        raise
```

- [ ] **Step 4: Run tests green**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_codex_adapter_parses_token_usage_events programbench_omp_adapter_parses_token_usage_events
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-bench/scripts/programbench_codex_agent.py crates/stateful-bench/scripts/programbench_omp_agent.py crates/stateful-bench/tests/programbench.rs
git commit -m "Add ProgramBench agent adapters"
```

---

### Task 5: Run and eval orchestration

**Files:**
- Modify: `crates/stateful-bench/src/programbench.rs`
- Modify: `crates/stateful-bench/src/lib.rs`
- Modify: `crates/stateful-bench/tests/programbench.rs`

**Interfaces:**
- Consumes: `ProgramBenchRunOptions`, `ProgramBenchRecipeCommand`, adapter scripts.
- Produces:
  - `ProgramBenchEvalOptions`
  - `planned_programbench_conditions(raw: &[String]) -> anyhow::Result<Vec<ProgramBenchCondition>>`
  - `build_programbench_eval_commands(options: ProgramBenchEvalOptions) -> anyhow::Result<Vec<ProgramBenchRecipeCommand>>`
  - `run_programbench_matrix(options: ProgramBenchRunOptions) -> anyhow::Result<Vec<ProgramBenchConditionMetadata>>`
  - `run_programbench_eval(options: ProgramBenchEvalOptions) -> anyhow::Result<()>`
  - CLI dispatch for all ProgramBench commands.

- [ ] **Step 1: Write failing orchestration tests**

Append to `crates/stateful-bench/tests/programbench.rs`:

```rust
use stateful_bench::{
    ProgramBenchEvalOptions, build_programbench_eval_commands, planned_programbench_conditions,
};

#[test]
fn programbench_run_uses_default_four_axis_matrix_when_no_conditions_passed() {
    let conditions = planned_programbench_conditions(&[]).expect("default conditions should build");
    assert_eq!(
        conditions.iter().map(ProgramBenchCondition::id).collect::<Vec<_>>(),
        vec![
            "stateful-off_subagent-off",
            "stateful-on_subagent-off",
            "stateful-off_subagent-on",
            "stateful-on_subagent-on",
        ]
    );
}

#[test]
fn programbench_eval_commands_run_eval_info_and_package_by_default() {
    let commands = build_programbench_eval_commands(ProgramBenchEvalOptions {
        run_dir: "runs/pb-dev".into(),
        programbench_bin: "programbench".to_string(),
        workers: 4,
        branch_workers: 2,
        docker_cpus: 8,
        force: true,
        no_package: false,
    })
    .expect("commands should build");
    let rendered = commands
        .iter()
        .map(|command| format!("{} {}", command.program, command.args.join(" ")))
        .collect::<Vec<_>>();
    assert_eq!(rendered[0], "programbench eval runs/pb-dev --workers 4 --branch-workers 2 --docker-cpus 8 --force");
    assert_eq!(rendered[1], "programbench info runs/pb-dev");
    assert_eq!(rendered[2], "programbench submit package runs/pb-dev");
}
```

- [ ] **Step 2: Run tests red**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_run_uses_default_four_axis_matrix_when_no_conditions_passed programbench_eval_commands_run_eval_info_and_package_by_default
```

Expected: FAIL because orchestration helpers do not exist.

- [ ] **Step 3: Implement orchestration helpers and dispatch**

Add `ProgramBenchEvalOptions`, `planned_programbench_conditions`, `build_programbench_eval_commands`, `run_programbench_matrix`, `run_programbench_eval`, `execute_recipe_command`, `write_json_file`, `read_json_file`, `write_or_print`, and `unix_ms` to `programbench.rs`. Implement `run_programbench_cli` so:

- `Run` converts raw condition strings with `planned_programbench_conditions`, creates `<output-dir>/<run-id>/conditions/<condition-id>/condition.json`, and prints metadata JSON.
- `Eval` runs `programbench eval`, `programbench info`, and package unless `--no-package` is set.
- `Report` builds a condition report and renders JSON or Markdown.
- `Compare` reads report JSON files, compares them, and renders JSON or Markdown.

The first implementation may create condition metadata and command plans without pulling ProgramBench instance lists inside unit tests. Actual Docker execution lives behind `execute_recipe_command` and adapter commands.

- [ ] **Step 4: Run tests green**

Run:

```bash
cargo test -p stateful-bench --test programbench programbench_run_uses_default_four_axis_matrix_when_no_conditions_passed programbench_eval_commands_run_eval_info_and_package_by_default
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/stateful-bench/src/lib.rs crates/stateful-bench/src/programbench.rs crates/stateful-bench/tests/programbench.rs
git commit -m "Wire ProgramBench run and eval orchestration"
```

---

### Task 6: Documentation and final verification

**Files:**
- Modify: `README.md:298-311`
- Modify: `docs/usage-reference.md:432-451`
- Create: `docs/programbench-benchmark-guide.md`

**Interfaces:**
- Consumes: finalized CLI and report fields.
- Produces: user-facing ProgramBench operating guide.

- [ ] **Step 1: Create ProgramBench guide**

Create `docs/programbench-benchmark-guide.md`:

```markdown
# ProgramBench Benchmark Guide

ProgramBench is a reverse-engineering benchmark: an agent receives a cleanroom container with a compiled `./executable` and bundled documentation, then writes an original source tree and `compile.sh` that rebuilds an equivalent executable.

## Runtime Requirements

- ProgramBench Docker images target Linux `amd64`.
- macOS developers can inspect commands and reports, but scored inference/evaluation needs a compatible Docker host.
- Install the official `programbench` CLI with `pip install programbench`, `uv pip install programbench`, or `uvx programbench`.

## Stateful Comparison Matrix

The default `stateful-bench programbench run` matrix is:

```text
stateful:off,subagent:off
stateful:on,subagent:off
stateful:off,subagent:on
stateful:on,subagent:on
```

Use the same instance set, model, image tag, max turns, timeout, and network policy across compared conditions.

## Inference Rules

ProgramBench inference remains offline by default. Agents must not search the internet, clone repositories, fetch target source from package registries, wrap the provided binary, decompile it, or run `strace`/`ltrace` on it. Agents may run `./executable` normally and read bundled documentation.

## Commands

Run a small Codex matrix:

```bash
stateful-bench programbench run \
  --run-id pb-dev \
  --agent codex-cli \
  --model gpt-5.4-mini \
  --filter 'ripgrep.*' \
  --max-instances 2
```

Evaluate with official ProgramBench tooling:

```bash
stateful-bench programbench eval \
  --run-dir .stateful_bench/programbench/runs/pb-dev \
  --workers 4 \
  --branch-workers 2 \
  --docker-cpus 8
```

Build a condition report:

```bash
stateful-bench programbench report \
  --condition-dir .stateful_bench/programbench/runs/pb-dev/conditions/stateful-on_subagent-off \
  --format markdown
```

Compare condition reports:

```bash
stateful-bench programbench compare \
  --report stateful-off_subagent-off.json \
  --report stateful-on_subagent-off.json \
  --format markdown
```

## Scoring

Official ProgramBench artifacts are the source of truth. `stateful-bench programbench eval` runs `programbench eval`, `programbench info`, and `programbench submit package` by default. Reports read `_stats/score.json` when present and label the score source.

## Efficiency Metrics

Reports include wall time, token totals, uncached token totals, subagent usage, score per million tokens, and score per hour. Treat quality and efficiency separately; a cheaper run that scores worse is not an automatic improvement.
```

- [ ] **Step 2: Update README benchmark summary**

Change README Benchmark Tooling around `README.md:298-311` to include ProgramBench:

```markdown
`stateful-bench` supports SWE-bench pair preparation/runs, reports, comparisons,
synthetic coordination experiments, DeNovoSWE adapters for official AweAgent,
host Codex CLI, and OMP CLI workflows, and ProgramBench stateful/no-state
condition runs with official ProgramBench evaluation and efficiency reporting.
```

Add `docs/programbench-benchmark-guide.md` to the Documentation list.

- [ ] **Step 3: Update usage-reference benchmark command list**

In `docs/usage-reference.md:432-451`, add:

```markdown
- `programbench`: run Codex or OMP agents on ProgramBench instances, evaluate the
  resulting `submission.tar.gz` artifacts with official ProgramBench tooling, and
  report stateful/no-state quality plus time/token efficiency deltas.
```

Add:

```markdown
For ProgramBench, scored runs require Linux `amd64` Docker support and official
`programbench eval`; see [ProgramBench Benchmark Guide](programbench-benchmark-guide.md).
```

- [ ] **Step 4: Run targeted tests**

Run:

```bash
cargo test -p stateful-bench --test programbench
```

Expected: PASS.

- [ ] **Step 5: Run formatting and broader crate tests**

Run:

```bash
cargo fmt --all --check
cargo test -p stateful-bench
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/usage-reference.md docs/programbench-benchmark-guide.md
git commit -m "Document ProgramBench benchmark workflow"
```

---

## Final Integration Checklist

- [ ] `cargo fmt --all --check` passes.
- [ ] `cargo test -p stateful-bench --test programbench` passes.
- [ ] `cargo test -p stateful-bench` passes.
- [ ] `README.md` mentions ProgramBench in Benchmark Tooling.
- [ ] `docs/usage-reference.md` lists `programbench` under Benchmark Commands.
- [ ] `docs/programbench-benchmark-guide.md` documents runtime constraints, default matrix, official scoring, and efficiency metrics.
- [ ] Each implementation task was committed separately.

## Plan Self-Review

- Spec coverage: tasks cover CLI, Codex/OMP adapters, stateful/no-state and subagent matrix, official eval/package scoring artifacts, report/compare efficiency metrics, docs, and Docker-free unit tests.
- Scan result: no unfinished markers, ellipsis markers, or undecided items remain.
- Type consistency: `ProgramBenchCondition`, `ProgramBenchTokenUsage`, `ProgramBenchConditionReport`, `ProgramBenchComparisonReport`, `ProgramBenchRunOptions`, `ProgramBenchEvalOptions`, and command-builder signatures are named consistently across tasks.
