use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ReportFormat;

const DEFAULT_DENOVO_CONFIG: &str = "configs/tasks/denovoswe.yaml";
const DEFAULT_DENOVO_EXTRACTS: &str = ".stateful_bench/denovo/extracts";
const DEFAULT_DENOVO_RUNS: &str = ".stateful_bench/denovo/runs";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum DeNovoRunMode {
    Batch,
    Single,
}

impl DeNovoRunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Batch => "batch",
            Self::Single => "single",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum DeNovoCommand {
    Extract {
        #[arg(long)]
        aweagent_root: Option<PathBuf>,
        #[arg(long, default_value = "python3")]
        python: String,
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = DEFAULT_DENOVO_EXTRACTS)]
        output: PathBuf,
        #[arg(long, default_value = DEFAULT_DENOVO_CONFIG)]
        config: PathBuf,
        #[arg(long)]
        max_concurrent: Option<usize>,
        #[arg(long = "instance-id")]
        instance_id: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        del_done_images: bool,
        #[arg(long)]
        no_extract_package_info: bool,
    },
    Run {
        #[arg(long)]
        aweagent_root: Option<PathBuf>,
        #[arg(long, default_value = "python3")]
        python: String,
        #[arg(long)]
        data_file: PathBuf,
        #[arg(long, default_value = DEFAULT_DENOVO_RUNS)]
        output_dir: PathBuf,
        #[arg(long, default_value_t = default_denovo_run_id())]
        run_id: String,
        #[arg(long, default_value = DEFAULT_DENOVO_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = DeNovoRunMode::Batch)]
        mode: DeNovoRunMode,
        #[arg(long)]
        condition: Vec<String>,
        #[arg(long)]
        llm_config: Option<PathBuf>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        max_steps: Option<usize>,
        #[arg(long)]
        max_concurrent: Option<usize>,
        #[arg(long = "instance-id")]
        instance_id: Vec<String>,
        #[arg(long, default_value_t = 1)]
        eval_iters: usize,
        #[arg(long, default_value = "v1")]
        prompt_version: String,
        #[arg(long)]
        enable_search: bool,
        #[arg(long)]
        no_search: bool,
        #[arg(long)]
        skip_eval: bool,
        #[arg(long)]
        validate_run: bool,
        #[arg(long)]
        del_done_images: bool,
        #[arg(long)]
        dump_clean_snapshot: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
    },
    Report {
        #[arg(long)]
        run_dir: PathBuf,
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

fn default_denovo_run_id() -> String {
    format!("denovo-{}", uuid::Uuid::new_v4())
}

pub fn search_override(enable_search: bool, no_search: bool) -> Option<bool> {
    match (enable_search, no_search) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

pub fn run_denovo_cli(command: DeNovoCommand) -> Result<()> {
    match command {
        DeNovoCommand::Extract { .. }
        | DeNovoCommand::Run { .. }
        | DeNovoCommand::Report { .. }
        | DeNovoCommand::Compare { .. } => {
            bail!("DeNovoSWE command execution is implemented in Tasks 4, 5, and 6")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeNovoExtractRecipeOptions {
    pub aweagent_root: PathBuf,
    pub python: String,
    pub input: PathBuf,
    pub output: PathBuf,
    pub config: PathBuf,
    pub max_concurrent: Option<usize>,
    pub instance_ids: Vec<String>,
    pub dry_run: bool,
    pub del_done_images: bool,
    pub no_extract_package_info: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeNovoRunRecipeOptions {
    pub aweagent_root: PathBuf,
    pub python: String,
    pub data_file: PathBuf,
    pub output: PathBuf,
    pub base_config: PathBuf,
    pub condition: DeNovoCondition,
    pub mode: DeNovoRunMode,
    pub instance_ids: Vec<String>,
    pub llm_config: Option<PathBuf>,
    pub model: Option<String>,
    pub max_steps: Option<usize>,
    pub max_concurrent: Option<usize>,
    pub search_override: Option<bool>,
    pub skip_eval: bool,
    pub validate_run: bool,
    pub eval_iters: usize,
    pub del_done_images: bool,
    pub dump_clean_snapshot: Option<PathBuf>,
    pub prompt_version: String,
    pub verbose: bool,
}

pub fn build_denovo_extract_recipe_command(
    options: DeNovoExtractRecipeOptions,
) -> Result<RecipeCommand> {
    let mut args = vec![
        "recipes/denovo_swe/extract_patch.py".to_string(),
        "--input".to_string(),
        path_arg(&options.input),
        "--output".to_string(),
        path_arg(&options.output),
        "--config".to_string(),
        path_arg(&options.config),
    ];
    push_optional_usize(&mut args, "--max-concurrent", options.max_concurrent);
    push_repeated(&mut args, "--instance-id", options.instance_ids);
    push_flag(&mut args, "--dry-run", options.dry_run);
    push_flag(&mut args, "--del-done-images", options.del_done_images);
    push_flag(
        &mut args,
        "--no-extract-package-info",
        options.no_extract_package_info,
    );

    Ok(RecipeCommand {
        program: options.python,
        args,
        cwd: options.aweagent_root,
        env: BTreeMap::new(),
    })
}

pub fn build_denovo_run_recipe_command(options: DeNovoRunRecipeOptions) -> Result<RecipeCommand> {
    let config = options
        .condition
        .config_path
        .as_ref()
        .unwrap_or(&options.base_config);
    let mut args = vec![
        "recipes/denovo_swe/run.py".to_string(),
        "--data-file".to_string(),
        path_arg(&options.data_file),
        "--config".to_string(),
        path_arg(config),
        "--mode".to_string(),
        options.mode.as_str().to_string(),
        "--output".to_string(),
        path_arg(&options.output),
        "--eval-iters".to_string(),
        options.eval_iters.to_string(),
        "--prompt-version".to_string(),
        options.prompt_version,
    ];
    push_optional_path(&mut args, "--llm-config", options.llm_config.as_ref());
    push_optional_string(&mut args, "--model", options.model.as_deref());
    push_optional_usize(&mut args, "--max-steps", options.max_steps);
    push_optional_usize(&mut args, "--max-concurrent", options.max_concurrent);
    push_repeated(&mut args, "--instance-id", options.instance_ids);
    match options.search_override {
        Some(true) => args.push("--enable-search".to_string()),
        Some(false) => args.push("--no-search".to_string()),
        None => {}
    }
    push_flag(&mut args, "--skip-eval", options.skip_eval);
    push_flag(&mut args, "--validate-run", options.validate_run);
    push_flag(&mut args, "--del-done-images", options.del_done_images);
    push_optional_path(
        &mut args,
        "--dump-clean-snapshot",
        options.dump_clean_snapshot.as_ref(),
    );
    push_flag(&mut args, "--verbose", options.verbose);

    Ok(RecipeCommand {
        program: options.python,
        args,
        cwd: options.aweagent_root,
        env: options.condition.env,
    })
}

fn push_flag(args: &mut Vec<String>, flag: &str, enabled: bool) {
    if enabled {
        args.push(flag.to_string());
    }
}

fn push_optional_usize(args: &mut Vec<String>, flag: &str, value: Option<usize>) {
    if let Some(value) = value {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

fn push_optional_path(args: &mut Vec<String>, flag: &str, value: Option<&PathBuf>) {
    if let Some(value) = value {
        args.push(flag.to_string());
        args.push(path_arg(value));
    }
}

fn push_optional_string(args: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

fn push_repeated(args: &mut Vec<String>, flag: &str, values: Vec<String>) {
    for value in values {
        args.push(flag.to_string());
        args.push(value);
    }
}

fn path_arg(path: &PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeNovoConditionRunOptions {
    pub run_id: String,
    pub aweagent_root: PathBuf,
    pub python: String,
    pub data_file: PathBuf,
    pub run_dir: PathBuf,
    pub base_config: PathBuf,
    pub condition: DeNovoCondition,
    pub mode: DeNovoRunMode,
    pub instance_ids: Vec<String>,
    pub llm_config: Option<PathBuf>,
    pub model: Option<String>,
    pub max_steps: Option<usize>,
    pub max_concurrent: Option<usize>,
    pub search_override: Option<bool>,
    pub skip_eval: bool,
    pub validate_run: bool,
    pub eval_iters: usize,
    pub del_done_images: bool,
    pub dump_clean_snapshot: Option<PathBuf>,
    pub prompt_version: String,
    pub verbose: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeNovoConditionMetadata {
    pub run_id: String,
    pub condition_id: String,
    pub condition: DeNovoCondition,
    pub command: RecipeCommand,
    pub official_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results_jsonl: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_json: Option<PathBuf>,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub running_time_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aweagent_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn run_denovo_condition(options: DeNovoConditionRunOptions) -> Result<DeNovoConditionMetadata> {
    let recipe = options.aweagent_root.join("recipes/denovo_swe/run.py");
    if !recipe.is_file() {
        bail!(
            "official DeNovoSWE run recipe not found at {}",
            recipe.display()
        );
    }

    let condition_id = options.condition.id();
    let condition_dir = options.run_dir.join("conditions").join(&condition_id);
    let official_dir = condition_dir.join("official");
    fs::create_dir_all(&official_dir)
        .with_context(|| format!("failed to create {}", official_dir.display()))?;

    let command = build_denovo_run_recipe_command(DeNovoRunRecipeOptions {
        aweagent_root: options.aweagent_root.clone(),
        python: options.python,
        data_file: options.data_file,
        output: official_dir.clone(),
        base_config: options.base_config,
        condition: options.condition.clone(),
        mode: options.mode,
        instance_ids: options.instance_ids,
        llm_config: options.llm_config,
        model: options.model,
        max_steps: options.max_steps,
        max_concurrent: options.max_concurrent,
        search_override: options.search_override,
        skip_eval: options.skip_eval,
        validate_run: options.validate_run,
        eval_iters: options.eval_iters,
        del_done_images: options.del_done_images,
        dump_clean_snapshot: options.dump_clean_snapshot,
        prompt_version: options.prompt_version,
        verbose: options.verbose,
    })?;

    let started_at_ms = unix_ms();
    let started = Instant::now();
    let execution = execute_recipe_command(&command);
    let running_time_ms = elapsed_ms(started);
    let finished_at_ms = unix_ms();
    let aweagent_commit = read_aweagent_commit(&options.aweagent_root);
    let metadata_path = condition_dir.join("condition.json");

    if let Err(error) = execution {
        let metadata = DeNovoConditionMetadata {
            run_id: options.run_id,
            condition_id,
            condition: options.condition,
            command,
            official_dir,
            results_jsonl: None,
            report_json: None,
            started_at_ms,
            finished_at_ms,
            running_time_ms,
            aweagent_commit,
            error: Some(error.to_string()),
        };
        write_json_file(&metadata_path, &metadata)?;
        return Err(error);
    }

    let results_jsonl = find_results_jsonl(&official_dir).with_context(|| {
        format!(
            "failed to locate results.jsonl under {}",
            official_dir.display()
        )
    })?;
    let results = crate::read_jsonl::<DeNovoOfficialResult>(&results_jsonl)?;
    let report = build_denovo_condition_report(
        options.run_id.clone(),
        options.condition.clone(),
        results,
        running_time_ms,
        aweagent_commit.clone(),
    );
    let report_json = condition_dir.join("denovo-report.json");
    write_json_file(&report_json, &report)?;

    let metadata = DeNovoConditionMetadata {
        run_id: options.run_id,
        condition_id,
        condition: options.condition,
        command,
        official_dir,
        results_jsonl: Some(results_jsonl),
        report_json: Some(report_json),
        started_at_ms,
        finished_at_ms,
        running_time_ms,
        aweagent_commit,
        error: None,
    };
    write_json_file(&metadata_path, &metadata)?;
    Ok(metadata)
}

fn execute_recipe_command(command: &RecipeCommand) -> Result<()> {
    let status = ProcessCommand::new(&command.program)
        .args(&command.args)
        .current_dir(&command.cwd)
        .envs(&command.env)
        .status()
        .with_context(|| format!("failed to execute {}", command_line(command)))?;
    if !status.success() {
        bail!(
            "official DeNovoSWE recipe failed with status {status}: {}",
            command_line(command)
        );
    }
    Ok(())
}

fn find_results_jsonl(official_dir: &Path) -> Result<PathBuf> {
    let preferred = official_dir.join("_").join("results.jsonl");
    if preferred.is_file() {
        return Ok(preferred);
    }
    find_file_named(official_dir, "results.jsonl").context("results.jsonl not found")
}

fn find_file_named(root: &Path, file_name: &str) -> Option<PathBuf> {
    let mut entries = fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_file() && path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_file_named(&path, file_name)
        {
            return Some(found);
        }
    }
    None
}

fn read_aweagent_commit(aweagent_root: &Path) -> Option<String> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(aweagent_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?;
    let commit = commit.trim();
    if commit.is_empty() {
        None
    } else {
        Some(commit.to_string())
    }
}

fn command_line(command: &RecipeCommand) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn unix_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

fn elapsed_ms(started: Instant) -> u64 {
    let millis = started.elapsed().as_millis().max(1);
    u64::try_from(millis).unwrap_or(u64::MAX)
}

fn write_json_file<T>(path: impl AsRef<Path>, value: &T) -> Result<()>
where
    T: Serialize,
{
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    serde_json::to_writer_pretty(file, value)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeNovoCondition {
    pub stateful: bool,
    pub subagent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl DeNovoCondition {
    pub fn new(stateful: bool, subagent: bool) -> Self {
        Self {
            stateful,
            subagent,
            config_path: None,
            env: BTreeMap::new(),
        }
    }

    pub fn id(&self) -> String {
        format!(
            "stateful-{}_subagent-{}",
            if self.stateful { "on" } else { "off" },
            if self.subagent { "on" } else { "off" }
        )
    }
}

pub fn default_denovo_conditions() -> Vec<DeNovoCondition> {
    vec![
        DeNovoCondition::new(false, false),
        DeNovoCondition::new(true, false),
        DeNovoCondition::new(false, true),
        DeNovoCondition::new(true, true),
    ]
}

pub fn parse_denovo_condition(input: &str) -> Result<DeNovoCondition> {
    let mut stateful = None;
    let mut subagent = None;
    let mut config_path = None;
    let mut env = BTreeMap::new();
    for raw_part in input.split(',') {
        let part = raw_part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((key, value)) = part.split_once(':') else {
            bail!("invalid DeNovoSWE condition part `{part}`; expected key:value");
        };
        match key {
            "stateful" => {
                if stateful
                    .replace(parse_axis(value).context("invalid stateful axis")?)
                    .is_some()
                {
                    bail!("duplicate DeNovoSWE condition key `stateful`");
                }
            }
            "subagent" => {
                if subagent
                    .replace(parse_axis(value).context("invalid subagent axis")?)
                    .is_some()
                {
                    bail!("duplicate DeNovoSWE condition key `subagent`");
                }
            }
            "config" => {
                if config_path.is_some() {
                    bail!("duplicate DeNovoSWE condition key `config`");
                }
                if value.is_empty() {
                    bail!("empty DeNovoSWE config path");
                }
                config_path = Some(PathBuf::from(value));
            }
            "env" => {
                let Some((env_key, env_value)) = value.split_once('=') else {
                    bail!("invalid DeNovoSWE env override `{value}`; expected KEY=VALUE");
                };
                if env_key.is_empty() {
                    bail!("empty DeNovoSWE env key");
                }
                if env_value.is_empty() {
                    bail!("empty DeNovoSWE env value for `{env_key}`");
                }
                if env
                    .insert(env_key.to_string(), env_value.to_string())
                    .is_some()
                {
                    bail!("duplicate DeNovoSWE env key `{env_key}`");
                }
            }
            other => bail!("unknown DeNovoSWE condition key `{other}`"),
        }
    }
    Ok(DeNovoCondition {
        stateful: stateful.context("missing stateful axis")?,
        subagent: subagent.context("missing subagent axis")?,
        config_path,
        env,
    })
}

fn parse_axis(value: &str) -> Result<bool> {
    match value {
        "on" | "true" => Ok(true),
        "off" | "false" => Ok(false),
        other => bail!("expected on/off, got `{other}`"),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeNovoOfficialResult {
    pub instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_result: Option<DeNovoEvalResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeNovoEvalResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<DeNovoEvalDetails>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeNovoEvalDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<u64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeNovoConditionReport {
    pub run_id: String,
    pub condition_id: String,
    pub condition: DeNovoCondition,
    pub total_instances: usize,
    pub completed_instances: usize,
    pub scored_instances: usize,
    pub pass_rate_instances: usize,
    pub success_count: usize,
    pub error_count: usize,
    pub success_rate: Option<f64>,
    pub average_score: Option<f64>,
    pub average_pass_rate: Option<f64>,
    pub correct_rate: Option<f64>,
    pub almost_correct_rate: Option<f64>,
    pub running_time_ms: u64,
    pub average_running_time_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aweagent_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeNovoComparisonReport {
    pub conditions: Vec<DeNovoConditionReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub duplicate_axis_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_axis_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub condition_id_mismatches: Vec<String>,
    pub stateful_score_delta_without_subagent: Option<f64>,
    pub subagent_score_delta_without_stateful: Option<f64>,
    pub combined_interaction_score_delta: Option<f64>,
    pub total_running_time_ms: u64,
}

pub fn build_denovo_condition_report(
    run_id: impl Into<String>,
    condition: DeNovoCondition,
    results: Vec<DeNovoOfficialResult>,
    running_time_ms: u64,
    aweagent_commit: Option<String>,
) -> DeNovoConditionReport {
    let total_instances = results.len();
    let completed_instances = results
        .iter()
        .filter(|result| result.error.is_none())
        .count();
    let success_count = results
        .iter()
        .filter(|result| result.success == Some(true))
        .count();
    let error_count = results
        .iter()
        .filter(|result| result.error.is_some())
        .count();
    let scores = results
        .iter()
        .filter_map(|result| result.score)
        .collect::<Vec<_>>();
    let pass_rates = results
        .iter()
        .filter_map(|result| {
            result
                .eval_result
                .as_ref()
                .and_then(|eval| eval.details.as_ref())
                .and_then(|details| details.pass_rate)
        })
        .collect::<Vec<_>>();
    let scored_instances = scores.len();
    let pass_rate_instances = pass_rates.len();
    let correct_count = scores.iter().filter(|score| **score >= 1.0).count();
    let almost_correct_count = scores
        .iter()
        .filter(|score| **score >= 0.75 && **score < 1.0)
        .count();

    let condition_id = condition.id();

    DeNovoConditionReport {
        run_id: run_id.into(),
        condition_id,
        condition,
        total_instances,
        completed_instances,
        scored_instances,
        pass_rate_instances,
        success_count,
        error_count,
        success_rate: ratio(success_count, total_instances),
        average_score: average(&scores),
        average_pass_rate: average(&pass_rates),
        correct_rate: ratio(correct_count, total_instances),
        almost_correct_rate: ratio(almost_correct_count, total_instances),
        running_time_ms,
        average_running_time_ms: if total_instances == 0 {
            None
        } else {
            Some(round_three(running_time_ms as f64 / total_instances as f64))
        },
        aweagent_commit,
    }
}

pub fn compare_denovo_reports(reports: Vec<DeNovoConditionReport>) -> DeNovoComparisonReport {
    let total_running_time_ms = reports
        .iter()
        .map(|report| report.running_time_ms)
        .sum::<u64>();
    let mut by_axes: BTreeMap<(bool, bool), Vec<&DeNovoConditionReport>> = BTreeMap::new();
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
        .map(|((stateful, subagent), _)| DeNovoCondition::new(*stateful, *subagent).id())
        .collect::<Vec<_>>();
    let missing_axis_ids = default_denovo_conditions()
        .into_iter()
        .filter(|condition| !by_axes.contains_key(&(condition.stateful, condition.subagent)))
        .map(|condition| condition.id())
        .collect::<Vec<_>>();

    let off_off = score_for(&by_axes, false, false);
    let on_off = score_for(&by_axes, true, false);
    let off_on = score_for(&by_axes, false, true);
    let on_on = score_for(&by_axes, true, true);

    DeNovoComparisonReport {
        conditions: reports,
        duplicate_axis_ids,
        missing_axis_ids,
        condition_id_mismatches,
        stateful_score_delta_without_subagent: delta(on_off, off_off),
        subagent_score_delta_without_stateful: delta(off_on, off_off),
        combined_interaction_score_delta: match (on_on, on_off, off_on, off_off) {
            (Some(on_on), Some(on_off), Some(off_on), Some(off_off)) => {
                Some(round_three(on_on - on_off - off_on + off_off))
            }
            _ => None,
        },
        total_running_time_ms,
    }
}

fn score_for(
    conditions: &BTreeMap<(bool, bool), Vec<&DeNovoConditionReport>>,
    stateful: bool,
    subagent: bool,
) -> Option<f64> {
    let reports = conditions.get(&(stateful, subagent))?;
    if reports.len() != 1 {
        return None;
    }
    let report = reports[0];
    if report.condition_id != report.condition.id() {
        return None;
    }
    report.average_score
}

fn delta(value: Option<f64>, baseline: Option<f64>) -> Option<f64> {
    Some(round_three(value? - baseline?))
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

fn round_three(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
