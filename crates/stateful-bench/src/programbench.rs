use std::{io, path::PathBuf, str::FromStr};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramBenchReportPath(PathBuf);

impl From<&str> for ProgramBenchReportPath {
    fn from(value: &str) -> Self {
        Self(PathBuf::from(value))
    }
}

impl FromStr for ProgramBenchReportPath {
    type Err = io::Error;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self(PathBuf::from(value)))
    }
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
        report: Vec<ProgramBenchReportPath>,
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

    pub fn id(&self) -> String {
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
