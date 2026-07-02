use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitStatus},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::ReportFormat;

const DEFAULT_PROGRAMBENCH_RUNS: &str = ".stateful_bench/programbench/runs";
const DEFAULT_PROGRAMBENCH_IMAGE_TAG: &str = "task_cleanroom_v6";
const DEFAULT_PROGRAMBENCH_BIN: &str = "programbench";
const DEFAULT_DOCKER_BIN: &str = "docker";
const DEFAULT_CODEX_BIN: &str = "codex";
const DEFAULT_OMP_BIN: &str = "omp";
const DEFAULT_PROGRAMBENCH_AGENT_DOCKER_OMP_BIN: &str = DEFAULT_OMP_BIN;
const DEFAULT_PROGRAMBENCH_AGENT_DOCKER_STATEFUL_BINARY: &str = "/usr/local/bin/stateful";
const DEFAULT_PROGRAMBENCH_AGENT_DOCKER_HOME: &str = "/home/stateful";
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
        #[arg(long)]
        agent_docker_image: Option<String>,
        #[arg(long, default_value = DEFAULT_PROGRAMBENCH_AGENT_DOCKER_OMP_BIN)]
        agent_docker_omp_bin: String,
        #[arg(long, default_value = DEFAULT_PROGRAMBENCH_AGENT_DOCKER_STATEFUL_BINARY)]
        agent_docker_stateful_binary: String,
        #[arg(long, default_value = DEFAULT_PROGRAMBENCH_AGENT_DOCKER_HOME)]
        agent_docker_home: String,
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
    pub agent_docker_image: Option<String>,
    pub agent_docker_omp_bin: String,
    pub agent_docker_stateful_binary: String,
    pub agent_docker_home: String,
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
    pub agent_docker_image: Option<String>,
    pub agent_docker_omp_bin: String,
    pub agent_docker_stateful_binary: String,
    pub agent_docker_home: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramBenchEvalOptions {
    pub run_dir: PathBuf,
    pub programbench_bin: String,
    pub workers: usize,
    pub branch_workers: usize,
    pub docker_cpus: usize,
    pub force: bool,
    pub no_package: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramBenchDiscoveredInstance {
    pub instance_id: String,
    pub image_name: String,
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
    pub archive_error: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramBenchInstanceReport {
    pub instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub running_time_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub token_input_plus_output_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub token_uncached_input_plus_output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_used: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramBenchConditionReport {
    pub run_id: String,
    pub condition_id: String,
    pub condition: ProgramBenchCondition,
    pub instances: usize,
    pub attempted_instances: usize,
    pub evaluated_instances: usize,
    pub average_score: Option<f64>,
    pub resolved_count: usize,
    pub resolved_rate: Option<f64>,
    pub eval_error_count: usize,
    pub agent_error_count: usize,
    pub timeout_count: usize,
    pub running_time_ms: u64,
    pub average_running_time_ms: Option<f64>,
    #[serde(default)]
    pub token_observed_instances: usize,
    #[serde(default)]
    pub token_usage_turns: usize,
    #[serde(default)]
    pub token_input_tokens: u64,
    #[serde(default)]
    pub token_cached_input_tokens: u64,
    #[serde(default)]
    pub token_output_tokens: u64,
    #[serde(default)]
    pub token_reasoning_output_tokens: u64,
    #[serde(default)]
    pub token_input_plus_output_tokens: u64,
    #[serde(default)]
    pub token_uncached_input_tokens: u64,
    #[serde(default)]
    pub token_uncached_input_plus_output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_input_plus_output_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_uncached_input_plus_output_tokens: Option<f64>,
    #[serde(default)]
    pub subagent_observed_instances: usize,
    #[serde(default)]
    pub subagent_used_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_used_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_per_million_input_plus_output_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_per_million_uncached_input_plus_output_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_per_hour: Option<f64>,
    pub score_source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instance_reports: Vec<ProgramBenchInstanceReport>,
}

impl ProgramBenchConditionReport {
    pub fn render(&self, format: ReportFormat) -> Result<String> {
        match format {
            ReportFormat::Json => serde_json::to_string_pretty(self)
                .context("failed to render ProgramBench report JSON"),
            ReportFormat::Markdown => Ok(render_programbench_report_markdown(self)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramBenchComparisonReport {
    pub reports: Vec<ProgramBenchConditionReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub duplicate_axis_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_axis_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub condition_id_mismatches: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instance_set_mismatches: Vec<String>,
    pub stateful_score_delta_without_subagent: Option<f64>,
    pub subagent_score_delta_without_stateful: Option<f64>,
    pub combined_interaction_score_delta: Option<f64>,
    pub stateful_running_time_ms_delta_without_subagent: Option<i64>,
    pub subagent_running_time_ms_delta_without_stateful: Option<i64>,
    pub stateful_input_plus_output_tokens_delta_without_subagent: Option<i64>,
    pub subagent_input_plus_output_tokens_delta_without_stateful: Option<i64>,
    pub total_running_time_ms: u64,
    #[serde(default)]
    pub total_input_plus_output_tokens: u64,
    #[serde(default)]
    pub total_uncached_input_plus_output_tokens: u64,
}

impl ProgramBenchComparisonReport {
    pub fn render(&self, format: ReportFormat) -> Result<String> {
        match format {
            ReportFormat::Json => serde_json::to_string_pretty(self)
                .context("failed to render ProgramBench comparison JSON"),
            ReportFormat::Markdown => Ok(render_programbench_comparison_markdown(self)),
        }
    }
}

pub fn build_programbench_condition_report(
    condition_dir: impl AsRef<Path>,
) -> Result<ProgramBenchConditionReport> {
    let condition_dir = condition_dir.as_ref();
    let metadata: ProgramBenchConditionMetadata =
        read_json_file(condition_dir.join("condition.json"))?;
    let score_by_instance: BTreeMap<String, BTreeMap<String, bool>> =
        read_json_file(condition_dir.join("_stats/score.json"))?;
    let attempted_instances = metadata.instances.len();
    let scores = metadata
        .instances
        .iter()
        .filter_map(|instance| instance_score(&score_by_instance, &instance.instance_id))
        .collect::<Vec<_>>();
    let evaluated_instances = scores.len();
    let resolved_count = scores.iter().filter(|score| **score >= 1.0).count();
    let eval_error_count = metadata
        .instances
        .iter()
        .filter(|instance| instance_score(&score_by_instance, &instance.instance_id).is_none())
        .count();
    let agent_error_count = metadata
        .instances
        .iter()
        .filter(|instance| instance.error.is_some())
        .count();
    let timeout_count = metadata
        .instances
        .iter()
        .filter(|instance| {
            instance
                .error
                .as_deref()
                .map(|error| error.to_ascii_lowercase().contains("timeout"))
                .unwrap_or(false)
        })
        .count();
    let subagent_observed_instances = metadata
        .instances
        .iter()
        .filter(|instance| instance.subagent_used.is_some())
        .count();
    let subagent_used_count = metadata
        .instances
        .iter()
        .filter(|instance| instance.subagent_used == Some(true))
        .count();
    let token_usages = metadata
        .instances
        .iter()
        .filter_map(|instance| instance.token_usage.as_ref())
        .filter(|usage| usage.has_observed_tokens())
        .collect::<Vec<_>>();
    let token_observed_instances = token_usages.len();
    let token_usage_turns = token_usages.iter().map(|usage| usage.turns).sum::<usize>();
    let token_input_tokens = token_usages
        .iter()
        .map(|usage| usage.input_tokens)
        .sum::<u64>();
    let token_cached_input_tokens = token_usages
        .iter()
        .map(|usage| usage.cached_input_tokens)
        .sum::<u64>();
    let token_output_tokens = token_usages
        .iter()
        .map(|usage| usage.output_tokens)
        .sum::<u64>();
    let token_reasoning_output_tokens = token_usages
        .iter()
        .map(|usage| usage.reasoning_output_tokens)
        .sum::<u64>();
    let token_input_plus_output_tokens = token_usages
        .iter()
        .map(|usage| usage.input_plus_output_tokens())
        .sum::<u64>();
    let token_uncached_input_tokens = token_usages
        .iter()
        .map(|usage| usage.uncached_input_tokens())
        .sum::<u64>();
    let token_uncached_input_plus_output_tokens = token_usages
        .iter()
        .map(|usage| usage.uncached_input_plus_output_tokens())
        .sum::<u64>();
    let instance_reports = metadata
        .instances
        .iter()
        .map(|instance| {
            let token_usage = instance
                .token_usage
                .as_ref()
                .filter(|usage| usage.has_observed_tokens());
            ProgramBenchInstanceReport {
                instance_id: instance.instance_id.clone(),
                score: instance_score(&score_by_instance, &instance.instance_id),
                running_time_ms: instance.running_time_ms,
                token_input_plus_output_tokens: token_usage
                    .map(|usage| usage.input_plus_output_tokens())
                    .unwrap_or(0),
                token_uncached_input_plus_output_tokens: token_usage
                    .map(|usage| usage.uncached_input_plus_output_tokens())
                    .unwrap_or(0),
                subagent_used: instance.subagent_used,
            }
        })
        .collect::<Vec<_>>();
    let average_score = average(&scores);

    Ok(ProgramBenchConditionReport {
        run_id: metadata.run_id,
        condition_id: metadata.condition_id,
        condition: metadata.condition,
        instances: attempted_instances,
        attempted_instances,
        evaluated_instances,
        average_score,
        resolved_count,
        resolved_rate: ratio(resolved_count, evaluated_instances),
        eval_error_count,
        agent_error_count,
        timeout_count,
        running_time_ms: metadata.running_time_ms,
        average_running_time_ms: average_u64(metadata.running_time_ms, attempted_instances),
        token_observed_instances,
        token_usage_turns,
        token_input_tokens,
        token_cached_input_tokens,
        token_output_tokens,
        token_reasoning_output_tokens,
        token_input_plus_output_tokens,
        token_uncached_input_tokens,
        token_uncached_input_plus_output_tokens,
        average_input_plus_output_tokens: average_u64(
            token_input_plus_output_tokens,
            token_observed_instances,
        ),
        average_uncached_input_plus_output_tokens: average_u64(
            token_uncached_input_plus_output_tokens,
            token_observed_instances,
        ),
        subagent_observed_instances,
        subagent_used_count,
        subagent_used_rate: ratio(subagent_used_count, subagent_observed_instances),
        score_per_million_input_plus_output_tokens: score_per_million(
            average_score,
            token_input_plus_output_tokens,
        ),
        score_per_million_uncached_input_plus_output_tokens: score_per_million(
            average_score,
            token_uncached_input_plus_output_tokens,
        ),
        score_per_hour: score_per_hour(average_score, metadata.running_time_ms),
        instance_reports,
        score_source: "score-json".to_string(),
    })
}

pub fn compare_programbench_reports(
    reports: Vec<ProgramBenchConditionReport>,
) -> ProgramBenchComparisonReport {
    let total_running_time_ms = reports
        .iter()
        .map(|report| report.running_time_ms)
        .sum::<u64>();
    let total_input_plus_output_tokens = reports
        .iter()
        .map(|report| report.token_input_plus_output_tokens)
        .sum::<u64>();
    let total_uncached_input_plus_output_tokens = reports
        .iter()
        .map(|report| report.token_uncached_input_plus_output_tokens)
        .sum::<u64>();
    let mut by_axes: BTreeMap<(bool, bool), Vec<&ProgramBenchConditionReport>> = BTreeMap::new();
    let mut condition_id_mismatches = Vec::new();
    for report in &reports {
        let expected_condition_id = report.condition.id();
        if report.condition_id != expected_condition_id {
            condition_id_mismatches.push(format!(
                "{} != {}",
                report.condition_id, expected_condition_id
            ));
        }
        by_axes
            .entry((report.condition.stateful, report.condition.subagent))
            .or_default()
            .push(report);
    }
    let duplicate_axis_ids = by_axes
        .iter()
        .filter(|(_, reports)| reports.len() > 1)
        .map(|((stateful, subagent), _)| ProgramBenchCondition::new(*stateful, *subagent).id())
        .collect::<Vec<_>>();
    let missing_axis_ids = default_programbench_conditions()
        .into_iter()
        .filter(|condition| !by_axes.contains_key(&(condition.stateful, condition.subagent)))
        .map(|condition| condition.id())
        .collect::<Vec<_>>();

    let mut instance_set_mismatches = Vec::new();
    let (stateful_without_subagent, stateful_mismatch) =
        common_instance_deltas(&by_axes, true, false, false, false);
    if let Some(mismatch) = stateful_mismatch {
        instance_set_mismatches.push(mismatch);
    }
    let (subagent_without_stateful, subagent_mismatch) =
        common_instance_deltas(&by_axes, false, true, false, false);
    if let Some(mismatch) = subagent_mismatch {
        instance_set_mismatches.push(mismatch);
    }
    let (combined_interaction_score_delta, combined_mismatch) =
        common_interaction_score_delta(&by_axes);
    if let Some(mismatch) = combined_mismatch {
        instance_set_mismatches.push(mismatch);
    }

    ProgramBenchComparisonReport {
        reports,
        duplicate_axis_ids,
        missing_axis_ids,
        condition_id_mismatches,
        instance_set_mismatches,
        stateful_score_delta_without_subagent: stateful_without_subagent.score_delta,
        subagent_score_delta_without_stateful: subagent_without_stateful.score_delta,
        combined_interaction_score_delta,
        stateful_running_time_ms_delta_without_subagent: stateful_without_subagent
            .running_time_ms_delta,
        subagent_running_time_ms_delta_without_stateful: subagent_without_stateful
            .running_time_ms_delta,
        stateful_input_plus_output_tokens_delta_without_subagent: stateful_without_subagent
            .input_plus_output_tokens_delta,
        subagent_input_plus_output_tokens_delta_without_stateful: subagent_without_stateful
            .input_plus_output_tokens_delta,
        total_running_time_ms,
        total_input_plus_output_tokens,
        total_uncached_input_plus_output_tokens,
    }
}

pub fn render_programbench_report_markdown(report: &ProgramBenchConditionReport) -> String {
    format!(
        "# ProgramBench Report\n\n| Condition | Stateful | Subagent | Instances | Evaluated | Partial score | Resolved | Running time ms | Input+output tokens | Uncached input+output tokens |\n| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n| {} | {} | {} | {} | {} | {} | {}/{} | {} | {} | {} |\n",
        report.condition_id,
        axis_label(report.condition.stateful),
        axis_label(report.condition.subagent),
        report.instances,
        report.evaluated_instances,
        optional_float(report.average_score),
        report.resolved_count,
        report.evaluated_instances,
        report.running_time_ms,
        report.token_input_plus_output_tokens,
        report.token_uncached_input_plus_output_tokens,
    )
}

pub fn render_programbench_comparison_markdown(report: &ProgramBenchComparisonReport) -> String {
    let mut output = String::from(
        "# ProgramBench Comparison\n\n| Condition | Stateful | Subagent | Instances | Partial score | Resolved | Running time ms | Input+output tokens |\n| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for condition_report in &report.reports {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {}/{} | {} | {} |\n",
            condition_report.condition_id,
            axis_label(condition_report.condition.stateful),
            axis_label(condition_report.condition.subagent),
            condition_report.instances,
            optional_float(condition_report.average_score),
            condition_report.resolved_count,
            condition_report.evaluated_instances,
            condition_report.running_time_ms,
            condition_report.token_input_plus_output_tokens,
        ));
    }
    output.push_str("\n## Deltas\n\n");
    output.push_str(&format!(
        "- Stateful score delta without subagent: {}\n",
        optional_float(report.stateful_score_delta_without_subagent)
    ));
    output.push_str(&format!(
        "- Stateful running time ms delta without subagent: {}\n",
        optional_i64(report.stateful_running_time_ms_delta_without_subagent)
    ));
    output.push_str(&format!(
        "- Stateful input+output token delta without subagent: {}\n",
        optional_i64(report.stateful_input_plus_output_tokens_delta_without_subagent)
    ));
    output.push_str(&format!(
        "- Subagent score delta without stateful: {}\n",
        optional_float(report.subagent_score_delta_without_stateful)
    ));
    output.push_str(&format!(
        "- Subagent running time ms delta without stateful: {}\n",
        optional_i64(report.subagent_running_time_ms_delta_without_stateful)
    ));
    output.push_str(&format!(
        "- Subagent input+output token delta without stateful: {}\n",
        optional_i64(report.subagent_input_plus_output_tokens_delta_without_stateful)
    ));
    output.push_str(&format!(
        "- Combined interaction score delta: {}\n",
        optional_float(report.combined_interaction_score_delta)
    ));
    if !report.missing_axis_ids.is_empty() {
        output.push_str(&format!(
            "- Missing axes: {}\n",
            report.missing_axis_ids.join(", ")
        ));
    }
    if !report.duplicate_axis_ids.is_empty() {
        output.push_str(&format!(
            "- Duplicate axes: {}\n",
            report.duplicate_axis_ids.join(", ")
        ));
    }
    if !report.condition_id_mismatches.is_empty() {
        output.push_str(&format!(
            "- Condition ID mismatches: {}\n",
            report.condition_id_mismatches.join(", ")
        ));
    }
    if !report.instance_set_mismatches.is_empty() {
        output.push_str(&format!(
            "- Instance set mismatches: {}\n",
            report.instance_set_mismatches.join("; ")
        ));
    }
    output
}

fn instance_score(
    scores: &BTreeMap<String, BTreeMap<String, bool>>,
    instance_id: &str,
) -> Option<f64> {
    let tests = scores.get(instance_id)?;
    if tests.is_empty() {
        return None;
    }
    let passed = tests.values().filter(|passed| **passed).count();
    Some(passed as f64 / tests.len() as f64)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct CommonInstanceDeltas {
    score_delta: Option<f64>,
    running_time_ms_delta: Option<i64>,
    input_plus_output_tokens_delta: Option<i64>,
}

fn common_instance_deltas(
    reports: &BTreeMap<(bool, bool), Vec<&ProgramBenchConditionReport>>,
    value_stateful: bool,
    value_subagent: bool,
    baseline_stateful: bool,
    baseline_subagent: bool,
) -> (CommonInstanceDeltas, Option<String>) {
    let Some(value_report) = report_for(reports, value_stateful, value_subagent) else {
        return (CommonInstanceDeltas::default(), None);
    };
    let Some(baseline_report) = report_for(reports, baseline_stateful, baseline_subagent) else {
        return (CommonInstanceDeltas::default(), None);
    };

    let value_instances = instance_reports_by_id(value_report);
    let baseline_instances = instance_reports_by_id(baseline_report);
    let common_ids = value_instances
        .keys()
        .filter(|instance_id| baseline_instances.contains_key(**instance_id))
        .copied()
        .collect::<Vec<_>>();
    let diagnostic = if common_ids.is_empty()
        || common_ids.len() != value_instances.len()
        || common_ids.len() != baseline_instances.len()
    {
        Some(format!(
            "{} vs {}: {} common instance(s), {} left-only, {} right-only",
            value_report.condition_id,
            baseline_report.condition_id,
            common_ids.len(),
            value_instances.len().saturating_sub(common_ids.len()),
            baseline_instances.len().saturating_sub(common_ids.len()),
        ))
    } else {
        None
    };
    if common_ids.is_empty() {
        return (CommonInstanceDeltas::default(), diagnostic);
    }

    let mut value_scores = Vec::new();
    let mut baseline_scores = Vec::new();
    let mut value_running_time_ms = 0_u64;
    let mut baseline_running_time_ms = 0_u64;
    let mut value_tokens = 0_u64;
    let mut baseline_tokens = 0_u64;
    for instance_id in common_ids {
        let value_instance = value_instances
            .get(instance_id)
            .expect("common instance should exist in value report");
        let baseline_instance = baseline_instances
            .get(instance_id)
            .expect("common instance should exist in baseline report");
        if let (Some(value_score), Some(baseline_score)) =
            (value_instance.score, baseline_instance.score)
        {
            value_scores.push(value_score);
            baseline_scores.push(baseline_score);
        }
        value_running_time_ms += value_instance.running_time_ms;
        baseline_running_time_ms += baseline_instance.running_time_ms;
        value_tokens += value_instance.token_input_plus_output_tokens;
        baseline_tokens += baseline_instance.token_input_plus_output_tokens;
    }

    (
        CommonInstanceDeltas {
            score_delta: delta(average(&value_scores), average(&baseline_scores)),
            running_time_ms_delta: delta_i64(
                Some(value_running_time_ms),
                Some(baseline_running_time_ms),
            ),
            input_plus_output_tokens_delta: delta_i64(Some(value_tokens), Some(baseline_tokens)),
        },
        diagnostic,
    )
}

fn common_interaction_score_delta(
    reports: &BTreeMap<(bool, bool), Vec<&ProgramBenchConditionReport>>,
) -> (Option<f64>, Option<String>) {
    let Some(off_off) = report_for(reports, false, false) else {
        return (None, None);
    };
    let Some(on_off) = report_for(reports, true, false) else {
        return (None, None);
    };
    let Some(off_on) = report_for(reports, false, true) else {
        return (None, None);
    };
    let Some(on_on) = report_for(reports, true, true) else {
        return (None, None);
    };
    let off_off_instances = instance_reports_by_id(off_off);
    let on_off_instances = instance_reports_by_id(on_off);
    let off_on_instances = instance_reports_by_id(off_on);
    let on_on_instances = instance_reports_by_id(on_on);
    let common_ids = off_off_instances
        .keys()
        .filter(|instance_id| {
            on_off_instances.contains_key(**instance_id)
                && off_on_instances.contains_key(**instance_id)
                && on_on_instances.contains_key(**instance_id)
        })
        .copied()
        .collect::<Vec<_>>();
    let diagnostic = if common_ids.is_empty()
        || common_ids.len() != off_off_instances.len()
        || common_ids.len() != on_off_instances.len()
        || common_ids.len() != off_on_instances.len()
        || common_ids.len() != on_on_instances.len()
    {
        Some(format!(
            "combined interaction: {} common instance(s) across {}, {}, {}, {}",
            common_ids.len(),
            off_off.condition_id,
            on_off.condition_id,
            off_on.condition_id,
            on_on.condition_id,
        ))
    } else {
        None
    };
    if common_ids.is_empty() {
        return (None, diagnostic);
    }

    let mut off_off_scores = Vec::new();
    let mut on_off_scores = Vec::new();
    let mut off_on_scores = Vec::new();
    let mut on_on_scores = Vec::new();
    for instance_id in common_ids {
        let off_off_instance = off_off_instances
            .get(instance_id)
            .expect("common instance should exist in off/off report");
        let on_off_instance = on_off_instances
            .get(instance_id)
            .expect("common instance should exist in on/off report");
        let off_on_instance = off_on_instances
            .get(instance_id)
            .expect("common instance should exist in off/on report");
        let on_on_instance = on_on_instances
            .get(instance_id)
            .expect("common instance should exist in on/on report");
        if let (Some(off_off_score), Some(on_off_score), Some(off_on_score), Some(on_on_score)) = (
            off_off_instance.score,
            on_off_instance.score,
            off_on_instance.score,
            on_on_instance.score,
        ) {
            off_off_scores.push(off_off_score);
            on_off_scores.push(on_off_score);
            off_on_scores.push(off_on_score);
            on_on_scores.push(on_on_score);
        }
    }

    let score_delta = match (
        average(&on_on_scores),
        average(&on_off_scores),
        average(&off_on_scores),
        average(&off_off_scores),
    ) {
        (Some(on_on), Some(on_off), Some(off_on), Some(off_off)) => {
            Some(round_three(on_on - on_off - off_on + off_off))
        }
        _ => None,
    };

    (score_delta, diagnostic)
}

fn instance_reports_by_id(
    report: &ProgramBenchConditionReport,
) -> BTreeMap<&str, &ProgramBenchInstanceReport> {
    report
        .instance_reports
        .iter()
        .map(|instance| (instance.instance_id.as_str(), instance))
        .collect()
}

fn report_for<'a>(
    reports: &'a BTreeMap<(bool, bool), Vec<&'a ProgramBenchConditionReport>>,
    stateful: bool,
    subagent: bool,
) -> Option<&'a ProgramBenchConditionReport> {
    let reports = reports.get(&(stateful, subagent))?;
    if reports.len() != 1 {
        return None;
    }
    let report = reports[0];
    if report.condition_id != report.condition.id() {
        return None;
    }
    Some(report)
}

fn score_per_million(score: Option<f64>, tokens: u64) -> Option<f64> {
    if tokens == 0 {
        None
    } else {
        Some(round_three(score? * 1_000_000.0 / tokens as f64))
    }
}

fn score_per_hour(score: Option<f64>, running_time_ms: u64) -> Option<f64> {
    if running_time_ms == 0 {
        None
    } else {
        Some(round_three(score? * 3_600_000.0 / running_time_ms as f64))
    }
}

fn delta(value: Option<f64>, baseline: Option<f64>) -> Option<f64> {
    Some(round_three(value? - baseline?))
}

fn delta_i64(value: Option<u64>, baseline: Option<u64>) -> Option<i64> {
    Some(value? as i64 - baseline? as i64)
}

fn ratio(numerator: usize, denominator: usize) -> Option<f64> {
    if denominator == 0 {
        None
    } else {
        Some(round_three(numerator as f64 / denominator as f64))
    }
}

fn average(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(round_three(
            values.iter().sum::<f64>() / values.len() as f64,
        ))
    }
}

fn average_u64(total: u64, count: usize) -> Option<f64> {
    if count == 0 {
        None
    } else {
        Some(round_three(total as f64 / count as f64))
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn round_three(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn optional_float(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn optional_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn read_json_file<T>(path: impl AsRef<Path>) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let path = path.as_ref();
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read JSON file {}", path.display()))?;
    serde_json::from_str(&input)
        .with_context(|| format!("failed to parse JSON file {}", path.display()))
}

pub fn default_programbench_conditions() -> Vec<ProgramBenchCondition> {
    vec![
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
        subagent.unwrap_or(true),
    ))
}

pub fn planned_programbench_conditions(raw: &[String]) -> Result<Vec<ProgramBenchCondition>> {
    if raw.is_empty() {
        return Ok(default_programbench_conditions());
    }
    raw.iter()
        .map(|condition| parse_programbench_condition(condition))
        .collect()
}

pub fn build_programbench_eval_commands(
    options: ProgramBenchEvalOptions,
) -> Result<Vec<ProgramBenchRecipeCommand>> {
    let condition_dirs = programbench_condition_dirs(&options.run_dir)?;
    let mut commands = Vec::new();
    for condition_dir in condition_dirs {
        let condition_dir = path_arg(&condition_dir);
        let mut eval_args = vec![
            "eval".to_string(),
            condition_dir.clone(),
            "--workers".to_string(),
            options.workers.to_string(),
            "--branch-workers".to_string(),
            options.branch_workers.to_string(),
            "--docker-cpus".to_string(),
            options.docker_cpus.to_string(),
        ];
        if options.force {
            eval_args.push("--force".to_string());
        }

        commands.push(ProgramBenchRecipeCommand {
            program: options.programbench_bin.clone(),
            args: eval_args,
            env: BTreeMap::new(),
        });
        commands.push(ProgramBenchRecipeCommand {
            program: options.programbench_bin.clone(),
            args: vec!["info".to_string(), condition_dir.clone()],
            env: BTreeMap::new(),
        });
        if !options.no_package {
            commands.push(ProgramBenchRecipeCommand {
                program: options.programbench_bin.clone(),
                args: vec!["submit".to_string(), "package".to_string(), condition_dir],
                env: BTreeMap::new(),
            });
        }
    }
    Ok(commands)
}

fn programbench_condition_dirs(run_dir: &Path) -> Result<Vec<PathBuf>> {
    let conditions_dir = run_dir.join("conditions");
    let mut condition_dirs = fs::read_dir(&conditions_dir)
        .with_context(|| format!("failed to read {}", conditions_dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("failed to list {}", conditions_dir.display()))?;
    condition_dirs.retain(|path| path.is_dir());
    condition_dirs.sort();
    if condition_dirs.is_empty() {
        bail!(
            "no ProgramBench condition directories found under {}",
            conditions_dir.display()
        );
    }
    Ok(condition_dirs)
}

pub fn run_programbench_matrix(
    options: ProgramBenchRunOptions,
) -> Result<Vec<ProgramBenchConditionMetadata>> {
    let instances = discover_programbench_instances(&options)?;
    run_programbench_matrix_with_instances(options, instances)
}

pub fn run_programbench_matrix_with_instances(
    options: ProgramBenchRunOptions,
    instances: Vec<ProgramBenchDiscoveredInstance>,
) -> Result<Vec<ProgramBenchConditionMetadata>> {
    if instances.is_empty() {
        bail!("no ProgramBench instances selected");
    }
    let conditions = if options.conditions.is_empty() {
        default_programbench_conditions()
    } else {
        options.conditions.clone()
    };
    let run_dir = options.output_dir.join(&options.run_id);
    let mut metadata = Vec::with_capacity(conditions.len());
    for condition in conditions {
        let condition_id = condition.id();
        let condition_dir = run_dir.join("conditions").join(&condition_id);
        fs::create_dir_all(&condition_dir)
            .with_context(|| format!("failed to create {}", condition_dir.display()))?;
        let started_at_ms = unix_ms();
        let mut instance_metadata = Vec::with_capacity(instances.len());
        for instance in &instances {
            instance_metadata.push(run_programbench_instance(
                &options,
                condition,
                &condition_dir,
                instance,
            )?);
        }
        let finished_at_ms = unix_ms();
        let condition_metadata = ProgramBenchConditionMetadata {
            run_id: options.run_id.clone(),
            condition_id,
            condition,
            agent: options.agent,
            started_at_ms,
            finished_at_ms,
            running_time_ms: finished_at_ms.saturating_sub(started_at_ms),
            instances: instance_metadata,
        };
        write_json_file(condition_dir.join("condition.json"), &condition_metadata)?;
        metadata.push(condition_metadata);
    }
    Ok(metadata)
}

fn discover_programbench_instances(
    options: &ProgramBenchRunOptions,
) -> Result<Vec<ProgramBenchDiscoveredInstance>> {
    let script = r#"
import argparse
import json

from programbench.utils.instance_filters import filter_instances
from programbench.utils.load_data import load_all_instances

parser = argparse.ArgumentParser()
parser.add_argument("--filter", default="")
parser.add_argument("--slice", default="")
parser.add_argument("--max-instances", type=int)
args = parser.parse_args()

instances = load_all_instances(include_tests=False)
instances = filter_instances(
    instances,
    filter_spec=args.filter,
    slice_spec=args.slice,
    shuffle=False,
)
if args.max_instances is not None:
    instances = instances[: args.max_instances]

print(json.dumps([
    {"instance_id": instance["instance_id"], "image_name": instance["image_name"]}
    for instance in instances
]))
"#;
    let mut command = programbench_python_command(&options.programbench_bin)?;
    command.arg("-c").arg(script);
    if let Some(filter) = &options.filter {
        command.arg("--filter").arg(filter);
    }
    if let Some(slice) = &options.slice {
        command.arg("--slice").arg(slice);
    }
    if let Some(max_instances) = options.max_instances {
        command
            .arg("--max-instances")
            .arg(max_instances.to_string());
    }
    let output = command
        .output()
        .context("failed to discover ProgramBench instances using Python API")?;
    if !output.status.success() {
        bail!(
            "failed to discover ProgramBench instances using Python API: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let instances: Vec<ProgramBenchDiscoveredInstance> = serde_json::from_slice(&output.stdout)
        .context("failed to parse ProgramBench instance discovery output")?;
    if instances.is_empty() {
        bail!("no ProgramBench instances selected");
    }
    Ok(instances)
}

fn programbench_python_command(programbench_bin: &str) -> Result<ProcessCommand> {
    let executable = resolve_programbench_executable(programbench_bin)?;
    let first_line = fs::read_to_string(&executable)
        .with_context(|| {
            format!(
                "failed to read ProgramBench executable {}",
                executable.display()
            )
        })?
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let shebang = first_line.strip_prefix("#!").ok_or_else(|| {
        anyhow::anyhow!(
            "ProgramBench executable {} is not a Python console script with a shebang",
            executable.display()
        )
    })?;
    let mut parts = shebang
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        bail!(
            "ProgramBench executable {} has an empty shebang",
            executable.display()
        );
    }
    let mut program = parts.remove(0);
    if program.ends_with("/env") {
        if parts.first().map(String::as_str) == Some("-S") {
            parts.remove(0);
        }
        if parts.is_empty() {
            bail!(
                "ProgramBench executable {} uses env without an interpreter",
                executable.display()
            );
        }
        program = parts.remove(0);
    }
    let mut command = ProcessCommand::new(program);
    command.args(parts);
    Ok(command)
}

fn resolve_programbench_executable(programbench_bin: &str) -> Result<PathBuf> {
    let path = Path::new(programbench_bin);
    if path.is_absolute() || path.components().count() > 1 {
        return Ok(path.to_path_buf());
    }
    let Some(paths) = env::var_os("PATH") else {
        bail!("failed to resolve ProgramBench executable `{programbench_bin}`: PATH is not set");
    };
    for dir in env::split_paths(&paths) {
        let candidate = dir.join(programbench_bin);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("failed to resolve ProgramBench executable `{programbench_bin}` in PATH")
}

fn run_programbench_instance(
    options: &ProgramBenchRunOptions,
    condition: ProgramBenchCondition,
    condition_dir: &Path,
    instance: &ProgramBenchDiscoveredInstance,
) -> Result<ProgramBenchInstanceMetadata> {
    let condition_id = condition.id();
    let container_name =
        programbench_container_name(&options.run_id, &condition_id, &instance.instance_id);
    let image = format!("{}:{}", instance.image_name, options.image_tag);
    let container_id = start_programbench_container(
        &options.docker_bin,
        &container_name,
        &image,
        options.timeout_seconds,
    )?;
    let command = build_programbench_agent_command(ProgramBenchInstanceRunOptions {
        agent: options.agent,
        condition,
        instance_id: instance.instance_id.clone(),
        container_id: container_id.clone(),
        condition_dir: condition_dir.to_path_buf(),
        docker_bin: options.docker_bin.clone(),
        codex_bin: options.codex_bin.clone(),
        omp_bin: options.omp_bin.clone(),
        stateful_binary: options.stateful_binary.clone(),
        model: options.model.clone(),
        benchmark_max_turns: options.benchmark_max_turns,
        timeout_seconds: options.timeout_seconds,
        subagent_min_count: 3,
        agent_docker_image: options.agent_docker_image.clone(),
        agent_docker_omp_bin: options.agent_docker_omp_bin.clone(),
        agent_docker_stateful_binary: options.agent_docker_stateful_binary.clone(),
        agent_docker_home: options.agent_docker_home.clone(),
    })?;
    let adapter_result = execute_recipe_command_status(&command);
    remove_programbench_container(&options.docker_bin, &container_id);
    adapter_result.with_context(|| format!("failed to execute {}", command_line(&command)))?;

    let metadata_path = condition_dir
        .join(&instance.instance_id)
        .join("instance.json");
    read_json_file(&metadata_path).with_context(|| {
        format!(
            "ProgramBench adapter did not write required metadata {}",
            metadata_path.display()
        )
    })
}

fn start_programbench_container(
    docker_bin: &str,
    container_name: &str,
    image: &str,
    _timeout_seconds: u64,
) -> Result<String> {
    let output = ProcessCommand::new(docker_bin)
        .args([
            "run",
            "-d",
            "--init",
            "--network",
            "none",
            "-w",
            "/workspace",
            "--name",
            container_name,
            image,
            "sleep",
            "infinity",
        ])
        .output()
        .with_context(|| format!("failed to start ProgramBench Docker container from {image}"))?;
    if !output.status.success() {
        bail!(
            "failed to start ProgramBench Docker container from {image}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if container_id.is_empty() {
        bail!("Docker did not return a ProgramBench container id for {image}");
    }
    Ok(container_id)
}

fn remove_programbench_container(docker_bin: &str, container_id: &str) {
    let _ = ProcessCommand::new(docker_bin)
        .args(["rm", "-f", container_id])
        .status();
}

fn programbench_container_name(run_id: &str, condition_id: &str, instance_id: &str) -> String {
    let raw = format!("stateful-bench-{run_id}-{condition_id}-{instance_id}");
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .take(96)
        .collect()
}

pub fn run_programbench_eval(options: ProgramBenchEvalOptions) -> Result<()> {
    for command in build_programbench_eval_commands(options)? {
        execute_recipe_command(&command)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ParentStatefulRuntimeFile {
    base_url: String,
    token: String,
}

fn inherit_parent_stateful_runtime_env(
    target_env: &mut BTreeMap<String, String>,
    source_env: &BTreeMap<String, String>,
) {
    let (Some(server_url), Some(server_token)) = (
        source_env.get("STATEFUL_SERVER_URL"),
        source_env.get("STATEFUL_SERVER_TOKEN"),
    ) else {
        return;
    };
    if server_url.is_empty() || server_token.is_empty() {
        return;
    }
    target_env.insert("STATEFUL_SERVER_URL".to_string(), server_url.clone());
    target_env.insert("STATEFUL_SERVER_TOKEN".to_string(), server_token.clone());
}

fn inherit_parent_stateful_runtime_env_from_file(
    target_env: &mut BTreeMap<String, String>,
    path: &Path,
) -> Result<()> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read Stateful runtime file {}", path.display()))?;
    let runtime: ParentStatefulRuntimeFile = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse Stateful runtime file {}", path.display()))?;
    let source_env = BTreeMap::from([
        ("STATEFUL_SERVER_URL".to_string(), runtime.base_url),
        ("STATEFUL_SERVER_TOKEN".to_string(), runtime.token),
    ]);
    inherit_parent_stateful_runtime_env(target_env, &source_env);
    Ok(())
}

fn parent_stateful_runtime_path(source_env: &BTreeMap<String, String>) -> Option<PathBuf> {
    if let Some(stateful_home) = source_env
        .get("STATEFUL_HOME")
        .filter(|value| !value.is_empty())
    {
        return Some(
            PathBuf::from(stateful_home)
                .join("runtime")
                .join("server.json"),
        );
    }
    source_env
        .get("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| {
            PathBuf::from(home)
                .join(".stateful_core")
                .join("runtime")
                .join("server.json")
        })
}

fn parent_stateful_runtime_env() -> BTreeMap<String, String> {
    let mut command_env = BTreeMap::new();
    let process_env = env::vars().collect::<BTreeMap<_, _>>();
    inherit_parent_stateful_runtime_env(&mut command_env, &process_env);
    if command_env.is_empty() {
        if let Some(runtime_path) = parent_stateful_runtime_path(&process_env) {
            let _ = inherit_parent_stateful_runtime_env_from_file(&mut command_env, &runtime_path);
        }
    }
    command_env
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
            if let Some(agent_docker_image) = options.agent_docker_image {
                args.push("--agent-docker-image".to_string());
                args.push(agent_docker_image);
                args.push("--agent-docker-omp-bin".to_string());
                args.push(options.agent_docker_omp_bin);
                args.push("--agent-docker-stateful-binary".to_string());
                args.push(options.agent_docker_stateful_binary);
                args.push("--agent-docker-home".to_string());
                args.push(options.agent_docker_home);
            }
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

    let env = if options.condition.stateful {
        parent_stateful_runtime_env()
    } else {
        BTreeMap::new()
    };

    Ok(ProgramBenchRecipeCommand {
        program: path_arg(&default_programbench_agent_script(options.agent)),
        args,
        env,
    })
}

pub fn run_programbench_cli(command: ProgramBenchCommand) -> Result<()> {
    match command {
        ProgramBenchCommand::Run {
            output_dir,
            run_id,
            agent,
            condition,
            model,
            benchmark_max_turns,
            timeout_seconds,
            filter,
            slice,
            max_instances,
            programbench_bin,
            docker_bin,
            image_tag,
            stateful_binary,
            codex_bin,
            omp_bin,
            agent_docker_image,
            agent_docker_omp_bin,
            agent_docker_stateful_binary,
            agent_docker_home,
        } => {
            let metadata = run_programbench_matrix(ProgramBenchRunOptions {
                output_dir,
                run_id,
                agent,
                conditions: planned_programbench_conditions(&condition)?,
                model,
                benchmark_max_turns,
                timeout_seconds,
                filter,
                slice,
                max_instances,
                programbench_bin,
                docker_bin,
                image_tag,
                stateful_binary,
                codex_bin,
                omp_bin,
                agent_docker_image,
                agent_docker_omp_bin,
                agent_docker_stateful_binary,
                agent_docker_home,
            })?;
            println!("{}", serde_json::to_string_pretty(&metadata)?);
        }
        ProgramBenchCommand::Eval {
            run_dir,
            programbench_bin,
            workers,
            branch_workers,
            docker_cpus,
            force,
            no_package,
        } => run_programbench_eval(ProgramBenchEvalOptions {
            run_dir,
            programbench_bin,
            workers,
            branch_workers,
            docker_cpus,
            force,
            no_package,
        })?,
        ProgramBenchCommand::Report {
            condition_dir,
            format,
            output,
        } => {
            let report = build_programbench_condition_report(condition_dir)?;
            let rendered = report.render(format)?;
            write_or_print(output.as_deref(), &rendered)?;
        }
        ProgramBenchCommand::Compare {
            report,
            format,
            output,
        } => {
            let reports = report
                .iter()
                .map(read_json_file::<ProgramBenchConditionReport>)
                .collect::<Result<Vec<_>>>()?;
            let comparison = compare_programbench_reports(reports);
            let rendered = comparison.render(format)?;
            write_or_print(output.as_deref(), &rendered)?;
        }
    }
    Ok(())
}

fn execute_recipe_command(command: &ProgramBenchRecipeCommand) -> Result<()> {
    let status = ProcessCommand::new(&command.program)
        .args(&command.args)
        .envs(&command.env)
        .status()
        .with_context(|| format!("failed to execute {}", command_line(command)))?;
    if !status.success() {
        bail!(
            "ProgramBench command failed with status {status}: {}",
            command_line(command)
        );
    }
    Ok(())
}

fn execute_recipe_command_status(command: &ProgramBenchRecipeCommand) -> Result<ExitStatus> {
    ProcessCommand::new(&command.program)
        .args(&command.args)
        .envs(&command.env)
        .status()
        .with_context(|| format!("failed to execute {}", command_line(command)))
}

fn command_line(command: &ProgramBenchRecipeCommand) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_json_file<T>(path: impl AsRef<Path>, value: &T) -> Result<()>
where
    T: Serialize,
{
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = serde_json::to_string_pretty(value)
        .with_context(|| format!("failed to render JSON for {}", path.display()))?;
    fs::write(path, rendered).with_context(|| format!("failed to write {}", path.display()))
}

fn write_or_print(output: Option<&Path>, rendered: &str) -> Result<()> {
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(output, rendered)
            .with_context(|| format!("failed to write {}", output.display()))?;
    } else {
        println!("{rendered}");
    }
    Ok(())
}

fn unix_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    u64::try_from(millis).unwrap_or(u64::MAX)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn inherit_parent_stateful_runtime_env_requires_url_and_token() {
        let mut target = BTreeMap::new();
        inherit_parent_stateful_runtime_env(
            &mut target,
            &env_map(&[("STATEFUL_SERVER_URL", "http://127.0.0.1:43873")]),
        );
        assert!(target.is_empty());

        inherit_parent_stateful_runtime_env(
            &mut target,
            &env_map(&[
                ("STATEFUL_SERVER_URL", "http://127.0.0.1:43873"),
                ("STATEFUL_SERVER_TOKEN", "parent-token"),
            ]),
        );
        assert_eq!(
            target.get("STATEFUL_SERVER_URL").map(String::as_str),
            Some("http://127.0.0.1:43873")
        );
        assert_eq!(
            target.get("STATEFUL_SERVER_TOKEN").map(String::as_str),
            Some("parent-token")
        );
    }

    #[test]
    fn inherit_parent_stateful_runtime_env_reads_runtime_file() {
        let root = env::temp_dir().join(format!(
            "stateful-bench-runtime-env-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        let runtime_path = root.join("server.json");
        fs::write(
            &runtime_path,
            r#"{
                "base_url": "http://127.0.0.1:43873",
                "token": "file-token",
                "pid": 123,
                "workspace_id": "workspace",
                "protocol_version": "stateful.v1",
                "started_at": "2026-06-29T00:00:00Z"
            }"#,
        )
        .expect("runtime file should be written");

        let mut target = BTreeMap::new();
        inherit_parent_stateful_runtime_env_from_file(&mut target, &runtime_path)
            .expect("runtime file should be read");

        assert_eq!(
            target.get("STATEFUL_SERVER_URL").map(String::as_str),
            Some("http://127.0.0.1:43873")
        );
        assert_eq!(
            target.get("STATEFUL_SERVER_TOKEN").map(String::as_str),
            Some("file-token")
        );
        fs::remove_dir_all(root).ok();
    }
}
