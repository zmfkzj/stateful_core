use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
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
}

impl ProgramBenchConditionReport {
    pub fn render(&self, format: ReportFormat) -> Result<String> {
        match format {
            ReportFormat::Json => serde_json::to_string_pretty(self).context("failed to render ProgramBench report JSON"),
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
    let metadata: ProgramBenchConditionMetadata = read_json_file(condition_dir.join("condition.json"))?;
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
            condition_id_mismatches.push(format!("{} != {}", report.condition_id, expected_condition_id));
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

    let off_off_score = score_for(&by_axes, false, false);
    let on_off_score = score_for(&by_axes, true, false);
    let off_on_score = score_for(&by_axes, false, true);
    let on_on_score = score_for(&by_axes, true, true);
    let off_off_time = running_time_for(&by_axes, false, false);
    let on_off_time = running_time_for(&by_axes, true, false);
    let off_on_time = running_time_for(&by_axes, false, true);
    let off_off_tokens = input_plus_output_tokens_for(&by_axes, false, false);
    let on_off_tokens = input_plus_output_tokens_for(&by_axes, true, false);
    let off_on_tokens = input_plus_output_tokens_for(&by_axes, false, true);

    ProgramBenchComparisonReport {
        reports,
        duplicate_axis_ids,
        missing_axis_ids,
        condition_id_mismatches,
        stateful_score_delta_without_subagent: delta(on_off_score, off_off_score),
        subagent_score_delta_without_stateful: delta(off_on_score, off_off_score),
        combined_interaction_score_delta: match (on_on_score, on_off_score, off_on_score, off_off_score) {
            (Some(on_on), Some(on_off), Some(off_on), Some(off_off)) => {
                Some(round_three(on_on - on_off - off_on + off_off))
            }
            _ => None,
        },
        stateful_running_time_ms_delta_without_subagent: delta_i64(on_off_time, off_off_time),
        subagent_running_time_ms_delta_without_stateful: delta_i64(off_on_time, off_off_time),
        stateful_input_plus_output_tokens_delta_without_subagent: delta_i64(
            on_off_tokens,
            off_off_tokens,
        ),
        subagent_input_plus_output_tokens_delta_without_stateful: delta_i64(
            off_on_tokens,
            off_off_tokens,
        ),
        total_running_time_ms,
        total_input_plus_output_tokens,
        total_uncached_input_plus_output_tokens,
    }
}

pub fn render_programbench_report_markdown(report: &ProgramBenchConditionReport) -> String {
    format!(
        "# ProgramBench Report\n\n| Condition | Stateful | Subagent | Instances | Evaluated | Average score | Resolved rate | Running time ms | Input+output tokens | Uncached input+output tokens |\n| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
        report.condition_id,
        axis_label(report.condition.stateful),
        axis_label(report.condition.subagent),
        report.instances,
        report.evaluated_instances,
        optional_float(report.average_score),
        optional_float(report.resolved_rate),
        report.running_time_ms,
        report.token_input_plus_output_tokens,
        report.token_uncached_input_plus_output_tokens,
    )
}

pub fn render_programbench_comparison_markdown(report: &ProgramBenchComparisonReport) -> String {
    let mut output = String::from(
        "# ProgramBench Comparison\n\n| Condition | Stateful | Subagent | Instances | Average score | Running time ms | Input+output tokens |\n| --- | --- | --- | ---: | ---: | ---: | ---: |\n",
    );
    for condition_report in &report.reports {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            condition_report.condition_id,
            axis_label(condition_report.condition.stateful),
            axis_label(condition_report.condition.subagent),
            condition_report.instances,
            optional_float(condition_report.average_score),
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
    if !report.missing_axis_ids.is_empty() {
        output.push_str(&format!("- Missing axes: {}\n", report.missing_axis_ids.join(", ")));
    }
    if !report.duplicate_axis_ids.is_empty() {
        output.push_str(&format!("- Duplicate axes: {}\n", report.duplicate_axis_ids.join(", ")));
    }
    if !report.condition_id_mismatches.is_empty() {
        output.push_str(&format!(
            "- Condition ID mismatches: {}\n",
            report.condition_id_mismatches.join(", ")
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

fn score_for(
    reports: &BTreeMap<(bool, bool), Vec<&ProgramBenchConditionReport>>,
    stateful: bool,
    subagent: bool,
) -> Option<f64> {
    let reports = reports.get(&(stateful, subagent))?;
    if reports.len() != 1 {
        return None;
    }
    let report = reports[0];
    if report.condition_id != report.condition.id() {
        return None;
    }
    report.average_score
}

fn running_time_for(
    reports: &BTreeMap<(bool, bool), Vec<&ProgramBenchConditionReport>>,
    stateful: bool,
    subagent: bool,
) -> Option<u64> {
    report_for(reports, stateful, subagent).map(|report| report.running_time_ms)
}

fn input_plus_output_tokens_for(
    reports: &BTreeMap<(bool, bool), Vec<&ProgramBenchConditionReport>>,
    stateful: bool,
    subagent: bool,
) -> Option<u64> {
    report_for(reports, stateful, subagent).map(|report| report.token_input_plus_output_tokens)
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
    let run_dir = path_arg(&options.run_dir);
    let mut eval_args = vec![
        "eval".to_string(),
        run_dir.clone(),
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

    let mut commands = vec![
        ProgramBenchRecipeCommand {
            program: options.programbench_bin.clone(),
            args: eval_args,
            env: BTreeMap::new(),
        },
        ProgramBenchRecipeCommand {
            program: options.programbench_bin.clone(),
            args: vec!["info".to_string(), run_dir.clone()],
            env: BTreeMap::new(),
        },
    ];
    if !options.no_package {
        commands.push(ProgramBenchRecipeCommand {
            program: options.programbench_bin,
            args: vec!["submit".to_string(), "package".to_string(), run_dir],
            env: BTreeMap::new(),
        });
    }
    Ok(commands)
}

pub fn run_programbench_matrix(
    options: ProgramBenchRunOptions,
) -> Result<Vec<ProgramBenchConditionMetadata>> {
    let conditions = if options.conditions.is_empty() {
        default_programbench_conditions()
    } else {
        options.conditions
    };
    let run_dir = options.output_dir.join(&options.run_id);
    let mut metadata = Vec::with_capacity(conditions.len());
    for condition in conditions {
        let condition_id = condition.id();
        let condition_dir = run_dir.join("conditions").join(&condition_id);
        fs::create_dir_all(&condition_dir)
            .with_context(|| format!("failed to create {}", condition_dir.display()))?;
        let started_at_ms = unix_ms();
        let finished_at_ms = unix_ms();
        let condition_metadata = ProgramBenchConditionMetadata {
            run_id: options.run_id.clone(),
            condition_id,
            condition,
            agent: options.agent,
            started_at_ms,
            finished_at_ms,
            running_time_ms: finished_at_ms.saturating_sub(started_at_ms),
            instances: Vec::new(),
        };
        write_json_file(condition_dir.join("condition.json"), &condition_metadata)?;
        metadata.push(condition_metadata);
    }
    Ok(metadata)
}

pub fn run_programbench_eval(options: ProgramBenchEvalOptions) -> Result<()> {
    for command in build_programbench_eval_commands(options)? {
        execute_recipe_command(&command)?;
    }
    Ok(())
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
