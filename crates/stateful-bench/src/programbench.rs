use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramBenchRecipeCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
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

    pub fn id(&self) -> String {
        format!(
            "stateful-{}_subagent-{}",
            axis_label(self.stateful),
            axis_label(self.subagent)
        )
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub subagent_used: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<ProgramBenchTokenUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

pub fn build_programbench_agent_command(
    options: ProgramBenchInstanceRunOptions,
) -> Result<ProgramBenchRecipeCommand> {
    let condition_id = options.condition.id();
    let mut args = vec![
        "--container-id".to_string(),
        options.container_id,
        "--instance-id".to_string(),
        options.instance_id,
        "--condition-id".to_string(),
        condition_id,
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
        ProgramBenchAgentKind::CodexCli => {
            args.push("--codex-bin".to_string());
            args.push(options.codex_bin);
        }
        ProgramBenchAgentKind::OmpCli => {
            args.push("--omp-bin".to_string());
            args.push(options.omp_bin);
        }
    }

    if options.condition.stateful {
        args.push("--stateful".to_string());
    }
    if options.condition.subagent {
        args.push("--subagent".to_string());
    }
    if let Some(model) = options.model {
        args.push("--model".to_string());
        args.push(model);
    }

    Ok(ProgramBenchRecipeCommand {
        program: path_arg(&default_programbench_agent_script(options.agent)),
        args,
        env: BTreeMap::new(),
    })
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

fn default_programbench_agent_script(agent: ProgramBenchAgentKind) -> PathBuf {
    let script = match agent {
        ProgramBenchAgentKind::CodexCli => "programbench_codex_agent.py",
        ProgramBenchAgentKind::OmpCli => "programbench_omp_agent.py",
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join(script)
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
