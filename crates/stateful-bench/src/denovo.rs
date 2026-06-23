use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeNovoCliRuntime {
    Codex,
    Omp,
}

impl DeNovoCliRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Omp => "omp",
        }
    }
}

const DEFAULT_CODEX_BIN: &str = "codex";
const DEFAULT_OMP_BIN: &str = "omp";
const DEFAULT_OMP_AGENT_DOCKER_STATEFUL_BINARY: &str = "/usr/local/bin/stateful";
const DEFAULT_CODEX_MODEL: &str = "gpt-5.4-mini";
const DEFAULT_OMP_MODEL: &str = "deepseek-v4-flash";
const DEFAULT_CODEX_REASONING_EFFORT: &str = "low";
const DEFAULT_CODEX_MODEL_CONTEXT_WINDOW: usize = 256000;
const DEFAULT_CODEX_TEMPERATURE: &str = "1";
const DEFAULT_CODEX_MAX_TURNS: usize = 500;
const DEFAULT_CODEX_SUBAGENT_MIN_COUNT: usize = 3;
const DEFAULT_CODEX_MAX_RESUMES: usize = 1;
const DEFAULT_CODEX_TIMEOUT_SECONDS: u64 = 7200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum DeNovoAgentKind {
    Official,
    CodexCli,
    OmpCli,
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
        #[arg(long, value_enum, default_value_t = DeNovoAgentKind::Official)]
        agent: DeNovoAgentKind,
        #[arg(long, default_value = DEFAULT_CODEX_BIN)]
        codex_bin: String,
        #[arg(long, default_value = DEFAULT_OMP_BIN)]
        omp_bin: String,
        #[arg(long, default_value = "stateful")]
        stateful_binary: String,
        #[arg(long)]
        agent_docker_image: Option<String>,
        #[arg(long, default_value = DEFAULT_OMP_AGENT_DOCKER_STATEFUL_BINARY)]
        agent_docker_stateful_binary: String,
        #[arg(long)]
        benchmark_model: Option<String>,
        #[arg(long, default_value = DEFAULT_CODEX_REASONING_EFFORT)]
        benchmark_reasoning_effort: String,
        #[arg(long, default_value_t = DEFAULT_CODEX_MODEL_CONTEXT_WINDOW)]
        benchmark_model_context_window: usize,
        #[arg(long, default_value = DEFAULT_CODEX_TEMPERATURE)]
        benchmark_temperature: String,
        #[arg(long, default_value_t = DEFAULT_CODEX_MAX_TURNS)]
        benchmark_max_turns: usize,
        #[arg(
            long,
            default_value_t = DEFAULT_CODEX_SUBAGENT_MIN_COUNT,
            value_parser = parse_positive_usize
        )]
        subagent_min_count: usize,
        #[arg(
            long,
            default_value_t = DEFAULT_CODEX_MAX_RESUMES,
            help = "Resume Codex only after context/token limit failures; official eval failures are not fed back to the agent"
        )]
        max_resumes: usize,
        #[arg(long, default_value_t = DEFAULT_CODEX_TIMEOUT_SECONDS)]
        codex_timeout_seconds: u64,
        #[arg(long)]
        codex_adapter_script: Option<PathBuf>,
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

fn parse_positive_usize(value: &str) -> std::result::Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|error| format!("expected a positive integer: {error}"))?;
    if parsed == 0 {
        return Err("expected a positive integer".to_string());
    }
    Ok(parsed)
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
        DeNovoCommand::Extract {
            aweagent_root,
            python,
            input,
            output,
            config,
            max_concurrent,
            instance_id,
            dry_run,
            del_done_images,
            no_extract_package_info,
        } => {
            let metadata = run_denovo_extract(DeNovoExtractOptions {
                aweagent_root: resolve_aweagent_root(aweagent_root)?,
                python,
                input,
                output,
                config,
                max_concurrent,
                instance_ids: instance_id,
                dry_run,
                del_done_images,
                no_extract_package_info,
            })?;
            println!("{}", serde_json::to_string_pretty(&metadata)?);
        }
        DeNovoCommand::Run {
            aweagent_root,
            python,
            data_file,
            output_dir,
            run_id,
            config,
            mode,
            condition,
            agent,
            codex_bin,
            omp_bin,
            stateful_binary,
            agent_docker_image,
            agent_docker_stateful_binary,
            benchmark_model,
            benchmark_reasoning_effort,
            benchmark_model_context_window,
            benchmark_temperature,
            benchmark_max_turns,
            subagent_min_count,
            max_resumes,
            codex_timeout_seconds,
            codex_adapter_script,
            llm_config,
            model,
            max_steps,
            max_concurrent,
            instance_id,
            eval_iters,
            prompt_version,
            enable_search,
            no_search,
            skip_eval,
            validate_run,
            del_done_images,
            dump_clean_snapshot,
            verbose,
        } => {
            let aweagent_root = resolve_aweagent_root(aweagent_root)?;
            let conditions = condition
                .iter()
                .map(|condition| parse_denovo_condition(condition))
                .collect::<Result<Vec<_>>>()?;
            let benchmark_model = benchmark_model.unwrap_or_else(|| match agent {
                DeNovoAgentKind::OmpCli => DEFAULT_OMP_MODEL.to_string(),
                _ => DEFAULT_CODEX_MODEL.to_string(),
            });
            let reports = run_denovo_matrix(DeNovoMatrixRunOptions {
                run_id: run_id.clone(),
                aweagent_root,
                python,
                data_file,
                run_dir: output_dir.join(run_id),
                base_config: config,
                conditions,
                agent,
                codex_bin,
                omp_bin,
                stateful_binary,
                agent_docker_image,
                agent_docker_stateful_binary,
                benchmark_model,
                benchmark_reasoning_effort,
                benchmark_model_context_window,
                benchmark_temperature,
                benchmark_max_turns,
                subagent_min_count,
                max_resumes,
                codex_timeout_seconds,
                codex_adapter_script,
                mode,
                instance_ids: instance_id,
                llm_config,
                model,
                max_steps,
                max_concurrent,
                search_override: search_override(enable_search, no_search),
                skip_eval,
                validate_run,
                eval_iters,
                del_done_images,
                dump_clean_snapshot,
                prompt_version,
                verbose,
            })?;
            println!("{}", serde_json::to_string_pretty(&reports)?);
        }
        DeNovoCommand::Report {
            run_dir,
            format,
            output,
        } => {
            let reports = read_condition_reports(&run_dir)?;
            let rendered = match format {
                ReportFormat::Json => serde_json::to_string_pretty(&reports)?,
                ReportFormat::Markdown => render_denovo_report_markdown(&reports),
            };
            write_or_print(output.as_deref(), &rendered)?;
        }
        DeNovoCommand::Compare {
            report,
            format,
            output,
        } => {
            let reports = report
                .iter()
                .map(read_json_file::<DeNovoConditionReport>)
                .collect::<Result<Vec<_>>>()?;
            let comparison = compare_denovo_reports(reports);
            let rendered = match format {
                ReportFormat::Json => serde_json::to_string_pretty(&comparison)?,
                ReportFormat::Markdown => render_denovo_comparison_markdown(&comparison),
            };
            write_or_print(output.as_deref(), &rendered)?;
        }
    }
    Ok(())
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeNovoCodexRunOptions {
    pub aweagent_root: PathBuf,
    pub python: String,
    pub data_file: PathBuf,
    pub output: PathBuf,
    pub base_config: PathBuf,
    pub condition: DeNovoCondition,
    pub mode: DeNovoRunMode,
    pub instance_ids: Vec<String>,
    pub max_steps: Option<usize>,
    pub max_concurrent: Option<usize>,
    pub skip_eval: bool,
    pub validate_run: bool,
    pub eval_iters: usize,
    pub del_done_images: bool,
    pub dump_clean_snapshot: Option<PathBuf>,
    pub prompt_version: String,
    pub verbose: bool,
    pub codex_bin: String,
    pub omp_bin: String,
    pub stateful_binary: String,
    pub agent_docker_image: Option<String>,
    pub agent_docker_stateful_binary: String,
    pub benchmark_model: String,
    pub benchmark_reasoning_effort: String,
    pub benchmark_model_context_window: usize,
    pub benchmark_temperature: String,
    pub benchmark_max_turns: usize,
    pub subagent_min_count: usize,
    pub max_resumes: usize,
    pub codex_timeout_seconds: u64,
    pub adapter_script: Option<PathBuf>,
    pub cli_runtime: DeNovoCliRuntime,
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

pub fn build_denovo_codex_adapter_command(options: DeNovoCodexRunOptions) -> Result<RecipeCommand> {
    let config = options
        .condition
        .config_path
        .as_ref()
        .unwrap_or(&options.base_config);
    let script = options
        .adapter_script
        .unwrap_or_else(default_denovo_codex_adapter_script);
    let agent_mode = if options.condition.stateful {
        "stateful"
    } else {
        "no-state"
    };
    let subagent = if options.condition.subagent {
        "on"
    } else {
        "off"
    };
    let mut args = vec![
        path_arg(&script),
        "--data-file".to_string(),
        path_arg(&options.data_file),
        "--config".to_string(),
        path_arg(config),
        "--mode".to_string(),
        options.mode.as_str().to_string(),
        "--output".to_string(),
        path_arg(&options.output),
        "--agent-mode".to_string(),
        agent_mode.to_string(),
        "--subagent".to_string(),
        subagent.to_string(),
        "--aweagent-root".to_string(),
        path_arg(&options.aweagent_root),
        "--cli-runtime".to_string(),
        options.cli_runtime.as_str().to_string(),
        "--codex-bin".to_string(),
        options.codex_bin,
        "--omp-bin".to_string(),
        options.omp_bin,
        "--stateful-binary".to_string(),
        options.stateful_binary,
        "--benchmark-model".to_string(),
        options.benchmark_model,
        "--benchmark-reasoning-effort".to_string(),
        options.benchmark_reasoning_effort,
        "--benchmark-model-context-window".to_string(),
        options.benchmark_model_context_window.to_string(),
        "--benchmark-temperature".to_string(),
        options.benchmark_temperature,
        "--benchmark-max-turns".to_string(),
        options.benchmark_max_turns.to_string(),
        "--subagent-min-count".to_string(),
        options.subagent_min_count.to_string(),
        "--max-resumes".to_string(),
        options.max_resumes.to_string(),
        "--codex-timeout-seconds".to_string(),
        options.codex_timeout_seconds.to_string(),
        "--eval-iters".to_string(),
        options.eval_iters.to_string(),
        "--prompt-version".to_string(),
        options.prompt_version,
    ];
    push_optional_string(
        &mut args,
        "--agent-docker-image",
        options.agent_docker_image.as_deref(),
    );
    if options.agent_docker_image.is_some() {
        args.push("--agent-docker-stateful-binary".to_string());
        args.push(options.agent_docker_stateful_binary);
    }
    push_optional_usize(&mut args, "--max-steps", options.max_steps);
    push_optional_usize(&mut args, "--max-concurrent", options.max_concurrent);
    push_repeated(&mut args, "--instance-id", options.instance_ids);
    push_flag(&mut args, "--skip-eval", options.skip_eval);
    push_flag(&mut args, "--validate-run", options.validate_run);
    if options.del_done_images {
        args.push("--del-done-images".to_string());
    } else {
        args.push("--keep-done-images".to_string());
    }
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

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn default_denovo_codex_adapter_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/denovo_codex_agent.py")
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
    pub agent: DeNovoAgentKind,
    pub codex_bin: String,
    pub omp_bin: String,
    pub stateful_binary: String,
    pub agent_docker_image: Option<String>,
    pub agent_docker_stateful_binary: String,
    pub benchmark_model: String,
    pub benchmark_reasoning_effort: String,
    pub benchmark_model_context_window: usize,
    pub benchmark_temperature: String,
    pub benchmark_max_turns: usize,
    pub subagent_min_count: usize,
    pub max_resumes: usize,
    pub codex_timeout_seconds: u64,
    pub codex_adapter_script: Option<PathBuf>,
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
    pub agent: DeNovoAgentKind,
    pub command: RecipeCommand,
    pub official_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results_jsonl: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_json: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_log: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_log: Option<PathBuf>,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub running_time_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aweagent_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn run_denovo_condition(options: DeNovoConditionRunOptions) -> Result<DeNovoConditionMetadata> {
    if options.agent == DeNovoAgentKind::Official {
        let recipe = options.aweagent_root.join("recipes/denovo_swe/run.py");
        if !recipe.is_file() {
            bail!(
                "official DeNovoSWE run recipe not found at {}",
                recipe.display()
            );
        }
    }

    let condition_id = options.condition.id();
    let condition_dir = options.run_dir.join("conditions").join(&condition_id);
    let agent_dir_name = match options.agent {
        DeNovoAgentKind::Official => "official",
        DeNovoAgentKind::CodexCli => "codex-cli",
        DeNovoAgentKind::OmpCli => "omp-cli",
    };
    let agent_output_dir = condition_dir.join(agent_dir_name);
    fs::create_dir_all(&agent_output_dir)
        .with_context(|| format!("failed to create {}", agent_output_dir.display()))?;
    let command_data_file = absolute_path(&options.data_file)?;
    let command_output_dir = absolute_path(&agent_output_dir)?;
    let command_adapter_script = options
        .codex_adapter_script
        .as_ref()
        .map(|path| absolute_path(path))
        .transpose()?;

    let command = match options.agent {
        DeNovoAgentKind::Official => build_denovo_run_recipe_command(DeNovoRunRecipeOptions {
            aweagent_root: options.aweagent_root.clone(),
            python: options.python,
            data_file: command_data_file,
            output: command_output_dir,
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
        }),
        DeNovoAgentKind::CodexCli | DeNovoAgentKind::OmpCli => {
            let cli_runtime = match options.agent {
                DeNovoAgentKind::OmpCli => DeNovoCliRuntime::Omp,
                _ => DeNovoCliRuntime::Codex,
            };
            build_denovo_codex_adapter_command(DeNovoCodexRunOptions {
                aweagent_root: options.aweagent_root.clone(),
                python: options.python,
                data_file: command_data_file,
                output: command_output_dir,
                base_config: options.base_config,
                condition: options.condition.clone(),
                mode: options.mode,
                instance_ids: options.instance_ids,
                max_steps: options.max_steps,
                max_concurrent: options.max_concurrent,
                skip_eval: options.skip_eval,
                validate_run: options.validate_run,
                eval_iters: options.eval_iters,
                del_done_images: options.del_done_images,
                dump_clean_snapshot: options.dump_clean_snapshot,
                prompt_version: options.prompt_version,
                verbose: options.verbose,
                codex_bin: options.codex_bin,
                omp_bin: options.omp_bin,
                stateful_binary: options.stateful_binary,
                agent_docker_image: options.agent_docker_image,
                agent_docker_stateful_binary: options.agent_docker_stateful_binary,
                benchmark_model: options.benchmark_model,
                benchmark_reasoning_effort: options.benchmark_reasoning_effort,
                benchmark_model_context_window: options.benchmark_model_context_window,
                benchmark_temperature: options.benchmark_temperature,
                benchmark_max_turns: options.benchmark_max_turns,
                subagent_min_count: options.subagent_min_count,
                max_resumes: options.max_resumes,
                codex_timeout_seconds: options.codex_timeout_seconds,
                adapter_script: command_adapter_script,
                cli_runtime,
            })
        }
    }?;

    let stdout_log = condition_dir.join("command.stdout.log");
    let stderr_log = condition_dir.join("command.stderr.log");
    let started_at_ms = unix_ms();
    let started = Instant::now();
    let execution = execute_recipe_command(&command, &stdout_log, &stderr_log);
    let running_time_ms = elapsed_ms(started);
    let finished_at_ms = unix_ms();
    let aweagent_commit = read_aweagent_commit(&options.aweagent_root);
    let metadata_path = condition_dir.join("condition.json");

    if let Err(error) = execution {
        let metadata = DeNovoConditionMetadata {
            run_id: options.run_id,
            condition_id,
            condition: options.condition,
            agent: options.agent,
            command,
            official_dir: agent_output_dir,
            results_jsonl: None,
            report_json: None,
            stdout_log: Some(stdout_log),
            stderr_log: Some(stderr_log),
            started_at_ms,
            finished_at_ms,
            running_time_ms,
            aweagent_commit,
            error: Some(error.to_string()),
        };
        write_json_file(&metadata_path, &metadata)?;
        return Err(error);
    }

    let results_jsonl = find_results_jsonl(&agent_output_dir).with_context(|| {
        format!(
            "failed to locate results.jsonl under {}",
            agent_output_dir.display()
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
        agent: options.agent,
        command,
        official_dir: agent_output_dir,
        results_jsonl: Some(results_jsonl),
        report_json: Some(report_json),
        stdout_log: Some(stdout_log),
        stderr_log: Some(stderr_log),
        started_at_ms,
        finished_at_ms,
        running_time_ms,
        aweagent_commit,
        error: None,
    };
    write_json_file(&metadata_path, &metadata)?;
    Ok(metadata)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeNovoMatrixRunOptions {
    pub run_id: String,
    pub aweagent_root: PathBuf,
    pub python: String,
    pub data_file: PathBuf,
    pub run_dir: PathBuf,
    pub base_config: PathBuf,
    pub conditions: Vec<DeNovoCondition>,
    pub agent: DeNovoAgentKind,
    pub codex_bin: String,
    pub omp_bin: String,
    pub stateful_binary: String,
    pub agent_docker_image: Option<String>,
    pub agent_docker_stateful_binary: String,
    pub benchmark_model: String,
    pub benchmark_reasoning_effort: String,
    pub benchmark_model_context_window: usize,
    pub benchmark_temperature: String,
    pub benchmark_max_turns: usize,
    pub subagent_min_count: usize,
    pub max_resumes: usize,
    pub codex_timeout_seconds: u64,
    pub codex_adapter_script: Option<PathBuf>,
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
struct DeNovoMatrixRunMetadata {
    run_id: String,
    run_dir: PathBuf,
    condition_ids: Vec<String>,
    started_at_ms: u64,
    finished_at_ms: u64,
    running_time_ms: u64,
}

#[derive(Debug, Clone)]
struct DeNovoConditionRunAggregate {
    condition: DeNovoCondition,
    command: Option<RecipeCommand>,
    official_dir: Option<PathBuf>,
    results: Vec<DeNovoOfficialResult>,
    started_at_ms: Option<u64>,
    finished_at_ms: Option<u64>,
    running_time_ms: u64,
    aweagent_commit: Option<String>,
}

fn flush_denovo_condition_aggregate(
    options: &DeNovoMatrixRunOptions,
    aggregate: &DeNovoConditionRunAggregate,
    started_at_ms: u64,
    write_metadata: bool,
) -> Result<DeNovoConditionReport> {
    let condition_id = aggregate.condition.id();
    let condition_dir = options.run_dir.join("conditions").join(&condition_id);
    let agent_dir_name = match options.agent {
        DeNovoAgentKind::Official => "official",
        DeNovoAgentKind::CodexCli => "codex-cli",
        DeNovoAgentKind::OmpCli => "omp-cli",
    };
    let agent_output_dir = aggregate
        .official_dir
        .clone()
        .unwrap_or_else(|| condition_dir.join(agent_dir_name));
    fs::create_dir_all(&agent_output_dir)
        .with_context(|| format!("failed to create {}", agent_output_dir.display()))?;
    let results_jsonl = agent_output_dir.join("_").join("results.jsonl");
    crate::write_jsonl(&results_jsonl, &aggregate.results)?;

    let report = build_denovo_condition_report(
        options.run_id.clone(),
        aggregate.condition.clone(),
        aggregate.results.clone(),
        aggregate.running_time_ms,
        aggregate.aweagent_commit.clone(),
    );
    let report_json = condition_dir.join("denovo-report.json");
    write_json_file(&report_json, &report)?;
    if write_metadata {
        if let Some(command) = aggregate.command.clone() {
            let metadata = DeNovoConditionMetadata {
                run_id: options.run_id.clone(),
                condition_id,
                condition: aggregate.condition.clone(),
                agent: options.agent,
                command,
                official_dir: agent_output_dir,
                results_jsonl: Some(results_jsonl),
                report_json: Some(report_json),
                stdout_log: Some(condition_dir.join("command.stdout.log")),
                stderr_log: Some(condition_dir.join("command.stderr.log")),
                started_at_ms: aggregate.started_at_ms.unwrap_or(started_at_ms),
                finished_at_ms: aggregate.finished_at_ms.unwrap_or_else(unix_ms),
                running_time_ms: aggregate.running_time_ms,
                aweagent_commit: aggregate.aweagent_commit.clone(),
                error: None,
            };
            write_json_file(condition_dir.join("condition.json"), &metadata)?;
        }
    }
    Ok(report)
}

fn flush_denovo_matrix_checkpoint(
    options: &DeNovoMatrixRunOptions,
    aggregates: &[DeNovoConditionRunAggregate],
    started_at_ms: u64,
    started: Instant,
    write_condition_metadata: bool,
) -> Result<Vec<DeNovoConditionReport>> {
    let mut reports = Vec::new();
    for aggregate in aggregates {
        let report = flush_denovo_condition_aggregate(
            options,
            aggregate,
            started_at_ms,
            write_condition_metadata,
        )?;
        reports.push(report);
    }

    let comparison = compare_denovo_reports(reports.clone());
    write_json_file(options.run_dir.join("comparison.json"), &comparison)?;
    let metadata = DeNovoMatrixRunMetadata {
        run_id: options.run_id.clone(),
        run_dir: options.run_dir.clone(),
        condition_ids: reports
            .iter()
            .map(|report| report.condition_id.clone())
            .collect(),
        started_at_ms,
        finished_at_ms: unix_ms(),
        running_time_ms: elapsed_ms(started),
    };
    write_json_file(options.run_dir.join("run.json"), &metadata)?;
    Ok(reports)
}

fn denovo_matrix_instance_ids(
    data_file: &Path,
    requested_instance_ids: &[String],
    mode: DeNovoRunMode,
) -> Result<Vec<String>> {
    let rows = crate::read_jsonl::<Value>(data_file)?;
    let mut instance_ids = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let instance_id = row
            .get("instance_id")
            .and_then(Value::as_str)
            .with_context(|| format!("DeNovoSWE data row {index} missing string instance_id"))?;
        if requested_instance_ids.is_empty()
            || requested_instance_ids
                .iter()
                .any(|requested| requested == instance_id)
        {
            instance_ids.push(instance_id.to_string());
        }
    }
    if mode == DeNovoRunMode::Single {
        instance_ids.truncate(1);
    }
    Ok(instance_ids)
}

pub fn run_denovo_matrix(options: DeNovoMatrixRunOptions) -> Result<Vec<DeNovoConditionReport>> {
    fs::create_dir_all(&options.run_dir)
        .with_context(|| format!("failed to create {}", options.run_dir.display()))?;
    let conditions = if options.conditions.is_empty() {
        default_denovo_conditions()
    } else {
        options.conditions.clone()
    };
    let started_at_ms = unix_ms();
    let started = Instant::now();
    let matrix_instance_ids =
        denovo_matrix_instance_ids(&options.data_file, &options.instance_ids, options.mode)?;
    let mut aggregates = conditions
        .iter()
        .cloned()
        .map(|condition| DeNovoConditionRunAggregate {
            condition,
            command: None,
            official_dir: None,
            results: Vec::new(),
            started_at_ms: None,
            finished_at_ms: None,
            running_time_ms: 0,
            aweagent_commit: None,
        })
        .collect::<Vec<_>>();

    let batch_cli_instances = matches!(
        options.agent,
        DeNovoAgentKind::CodexCli | DeNovoAgentKind::OmpCli
    ) && options.max_concurrent.unwrap_or(1) > 1;
    if batch_cli_instances {
        let last_condition_index = aggregates.len().saturating_sub(1);
        for condition_index in 0..aggregates.len() {
            let condition = aggregates[condition_index].condition.clone();
            let metadata = match run_denovo_condition(DeNovoConditionRunOptions {
                run_id: options.run_id.clone(),
                aweagent_root: options.aweagent_root.clone(),
                python: options.python.clone(),
                data_file: options.data_file.clone(),
                run_dir: options.run_dir.clone(),
                base_config: options.base_config.clone(),
                condition,
                agent: options.agent,
                codex_bin: options.codex_bin.clone(),
                omp_bin: options.omp_bin.clone(),
                stateful_binary: options.stateful_binary.clone(),
                agent_docker_image: options.agent_docker_image.clone(),
                agent_docker_stateful_binary: options.agent_docker_stateful_binary.clone(),
                benchmark_model: options.benchmark_model.clone(),
                benchmark_reasoning_effort: options.benchmark_reasoning_effort.clone(),
                benchmark_model_context_window: options.benchmark_model_context_window,
                benchmark_temperature: options.benchmark_temperature.clone(),
                benchmark_max_turns: options.benchmark_max_turns,
                subagent_min_count: options.subagent_min_count,
                max_resumes: options.max_resumes,
                codex_timeout_seconds: options.codex_timeout_seconds,
                codex_adapter_script: options.codex_adapter_script.clone(),
                mode: options.mode,
                instance_ids: matrix_instance_ids.clone(),
                llm_config: options.llm_config.clone(),
                model: options.model.clone(),
                max_steps: options.max_steps,
                max_concurrent: options.max_concurrent,
                search_override: options.search_override,
                skip_eval: options.skip_eval,
                validate_run: options.validate_run,
                eval_iters: options.eval_iters,
                del_done_images: options.del_done_images && condition_index == last_condition_index,
                dump_clean_snapshot: options.dump_clean_snapshot.clone(),
                prompt_version: options.prompt_version.clone(),
                verbose: options.verbose,
            }) {
                Ok(metadata) => metadata,
                Err(error) => {
                    flush_denovo_matrix_checkpoint(
                        &options,
                        &aggregates,
                        started_at_ms,
                        started,
                        false,
                    )?;
                    return Err(error);
                }
            };
            let aggregate = &mut aggregates[condition_index];
            if let Some(results_jsonl) = metadata.results_jsonl.as_ref() {
                aggregate
                    .results
                    .extend(crate::read_jsonl::<DeNovoOfficialResult>(results_jsonl)?);
            }
            aggregate.started_at_ms = Some(metadata.started_at_ms);
            aggregate.finished_at_ms = Some(metadata.finished_at_ms);
            aggregate.running_time_ms += metadata.running_time_ms;
            if aggregate.aweagent_commit.is_none() {
                aggregate.aweagent_commit = metadata.aweagent_commit.clone();
            }
            aggregate.command = Some(metadata.command);
            aggregate.official_dir = Some(metadata.official_dir);
            flush_denovo_condition_aggregate(&options, aggregate, started_at_ms, true)?;
            flush_denovo_matrix_checkpoint(&options, &aggregates, started_at_ms, started, true)?;
        }

        let reports =
            flush_denovo_matrix_checkpoint(&options, &aggregates, started_at_ms, started, true)?;
        return Ok(reports);
    }

    let last_condition_index = aggregates.len().saturating_sub(1);
    for instance_id in &matrix_instance_ids {
        for condition_index in 0..aggregates.len() {
            let condition = aggregates[condition_index].condition.clone();
            let metadata = match run_denovo_condition(DeNovoConditionRunOptions {
                run_id: options.run_id.clone(),
                aweagent_root: options.aweagent_root.clone(),
                python: options.python.clone(),
                data_file: options.data_file.clone(),
                run_dir: options.run_dir.clone(),
                base_config: options.base_config.clone(),
                condition,
                agent: options.agent,
                codex_bin: options.codex_bin.clone(),
                omp_bin: options.omp_bin.clone(),
                stateful_binary: options.stateful_binary.clone(),
                agent_docker_image: options.agent_docker_image.clone(),
                agent_docker_stateful_binary: options.agent_docker_stateful_binary.clone(),
                benchmark_model: options.benchmark_model.clone(),
                benchmark_reasoning_effort: options.benchmark_reasoning_effort.clone(),
                benchmark_model_context_window: options.benchmark_model_context_window,
                benchmark_temperature: options.benchmark_temperature.clone(),
                benchmark_max_turns: options.benchmark_max_turns,
                subagent_min_count: options.subagent_min_count,
                max_resumes: options.max_resumes,
                codex_timeout_seconds: options.codex_timeout_seconds,
                codex_adapter_script: options.codex_adapter_script.clone(),
                mode: options.mode,
                instance_ids: vec![instance_id.clone()],
                llm_config: options.llm_config.clone(),
                model: options.model.clone(),
                max_steps: options.max_steps,
                max_concurrent: options.max_concurrent,
                search_override: options.search_override,
                skip_eval: options.skip_eval,
                validate_run: options.validate_run,
                eval_iters: options.eval_iters,
                del_done_images: options.del_done_images && condition_index == last_condition_index,
                dump_clean_snapshot: options.dump_clean_snapshot.clone(),
                prompt_version: options.prompt_version.clone(),
                verbose: options.verbose,
            }) {
                Ok(metadata) => metadata,
                Err(error) => {
                    flush_denovo_matrix_checkpoint(
                        &options,
                        &aggregates,
                        started_at_ms,
                        started,
                        false,
                    )?;
                    return Err(error);
                }
            };
            let aggregate = &mut aggregates[condition_index];
            if let Some(results_jsonl) = metadata.results_jsonl.as_ref() {
                aggregate
                    .results
                    .extend(crate::read_jsonl::<DeNovoOfficialResult>(results_jsonl)?);
            }
            aggregate.started_at_ms = Some(
                aggregate
                    .started_at_ms
                    .map_or(metadata.started_at_ms, |started| {
                        started.min(metadata.started_at_ms)
                    }),
            );
            aggregate.finished_at_ms = Some(
                aggregate
                    .finished_at_ms
                    .map_or(metadata.finished_at_ms, |finished| {
                        finished.max(metadata.finished_at_ms)
                    }),
            );
            aggregate.running_time_ms += metadata.running_time_ms;
            if aggregate.aweagent_commit.is_none() {
                aggregate.aweagent_commit = metadata.aweagent_commit.clone();
            }
            aggregate.command = Some(metadata.command);
            aggregate.official_dir = Some(metadata.official_dir);
            flush_denovo_condition_aggregate(&options, aggregate, started_at_ms, true)?;
        }
        flush_denovo_matrix_checkpoint(&options, &aggregates, started_at_ms, started, true)?;
    }

    let reports =
        flush_denovo_matrix_checkpoint(&options, &aggregates, started_at_ms, started, true)?;
    Ok(reports)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeNovoExtractOptions {
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeNovoExtractMetadata {
    pub aweagent_root: PathBuf,
    pub output: PathBuf,
    pub command: RecipeCommand,
    pub results_jsonl: PathBuf,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub running_time_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aweagent_commit: Option<String>,
}

pub fn run_denovo_extract(options: DeNovoExtractOptions) -> Result<DeNovoExtractMetadata> {
    let recipe = options
        .aweagent_root
        .join("recipes/denovo_swe/extract_patch.py");
    if !recipe.is_file() {
        bail!(
            "official DeNovoSWE extract recipe not found at {}",
            recipe.display()
        );
    }
    fs::create_dir_all(&options.output)
        .with_context(|| format!("failed to create {}", options.output.display()))?;
    let command_input = absolute_path(&options.input)?;
    let command_output = absolute_path(&options.output)?;
    let command_config = absolute_path(&options.config)?;

    let command = build_denovo_extract_recipe_command(DeNovoExtractRecipeOptions {
        aweagent_root: options.aweagent_root.clone(),
        python: options.python,
        input: command_input,
        output: command_output,
        config: command_config,
        max_concurrent: options.max_concurrent,
        instance_ids: options.instance_ids,
        dry_run: options.dry_run,
        del_done_images: options.del_done_images,
        no_extract_package_info: options.no_extract_package_info,
    })?;
    let started_at_ms = unix_ms();
    let started = Instant::now();
    let stdout_log = options.output.join("command.stdout.log");
    let stderr_log = options.output.join("command.stderr.log");
    execute_recipe_command(&command, &stdout_log, &stderr_log)?;
    let running_time_ms = elapsed_ms(started);
    let finished_at_ms = unix_ms();
    let results_jsonl = find_results_jsonl(&options.output).with_context(|| {
        format!(
            "failed to locate extract results.jsonl under {}",
            options.output.display()
        )
    })?;
    let metadata = DeNovoExtractMetadata {
        aweagent_root: options.aweagent_root.clone(),
        output: options.output.clone(),
        command,
        results_jsonl,
        started_at_ms,
        finished_at_ms,
        running_time_ms,
        aweagent_commit: read_aweagent_commit(&options.aweagent_root),
    };
    write_json_file(options.output.join("denovo-extract.json"), &metadata)?;
    Ok(metadata)
}

pub fn render_denovo_report_markdown(reports: &[DeNovoConditionReport]) -> String {
    let mut output = String::from(
        "# DeNovoSWE Report\n\n| Condition | Stateful | Subagent | Instances | Success rate | Average score | Running time ms | Input+output tokens | Uncached input+output tokens |\n| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for report in reports {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            report.condition_id,
            axis_label(report.condition.stateful),
            axis_label(report.condition.subagent),
            report.total_instances,
            optional_float(report.success_rate),
            optional_float(report.average_score),
            report.running_time_ms,
            report.token_input_plus_output_tokens,
            report.token_uncached_input_plus_output_tokens
        ));
    }
    output
}

pub fn render_denovo_comparison_markdown(report: &DeNovoComparisonReport) -> String {
    let mut output = render_denovo_report_markdown(&report.conditions);
    output.push_str("\n## Comparison\n\n");
    output.push_str(&format!(
        "- Stateful delta without subagent: {}\n",
        optional_float(report.stateful_score_delta_without_subagent)
    ));
    output.push_str(&format!(
        "- Subagent delta without stateful: {}\n",
        optional_float(report.subagent_score_delta_without_stateful)
    ));
    output.push_str(&format!(
        "- Combined interaction delta: {}\n",
        optional_float(report.combined_interaction_score_delta)
    ));
    output.push_str(&format!(
        "- Total running time ms: {}\n",
        report.total_running_time_ms
    ));
    output.push_str(&format!(
        "- Total input+output tokens: {}\n",
        report.total_input_plus_output_tokens
    ));
    output.push_str(&format!(
        "- Total uncached input+output tokens: {}\n",
        report.total_uncached_input_plus_output_tokens
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
    output
}

fn execute_recipe_command(
    command: &RecipeCommand,
    stdout_log: &Path,
    stderr_log: &Path,
) -> Result<()> {
    let stdout = File::create(stdout_log)
        .with_context(|| format!("failed to create stdout log {}", stdout_log.display()))?;
    let stderr = File::create(stderr_log)
        .with_context(|| format!("failed to create stderr log {}", stderr_log.display()))?;
    let status = ProcessCommand::new(&command.program)
        .args(&command.args)
        .current_dir(&command.cwd)
        .envs(&command.env)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .status()
        .with_context(|| {
            format!(
                "failed to execute {}; stdout log: {}; stderr log: {}",
                command_line(command),
                stdout_log.display(),
                stderr_log.display()
            )
        })?;
    if !status.success() {
        bail!(
            "DeNovoSWE command failed with status {status}: {}; stdout log: {}; stderr log: {}",
            command_line(command),
            stdout_log.display(),
            stderr_log.display()
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

fn resolve_aweagent_root(aweagent_root: Option<PathBuf>) -> Result<PathBuf> {
    let root = match aweagent_root {
        Some(root) => root,
        None => env::var_os("AWEAGENT_ROOT")
            .map(PathBuf::from)
            .context("provide --aweagent-root or set AWEAGENT_ROOT")?,
    };
    if !root.is_dir() {
        bail!("AweAgent root does not exist: {}", root.display());
    }
    Ok(root)
}

fn read_condition_reports(run_dir: &Path) -> Result<Vec<DeNovoConditionReport>> {
    let conditions_dir = run_dir.join("conditions");
    let mut report_paths = fs::read_dir(&conditions_dir)
        .with_context(|| format!("failed to read {}", conditions_dir.display()))?
        .filter_map(|entry| {
            entry
                .ok()
                .map(|entry| entry.path().join("denovo-report.json"))
        })
        .filter(|path| path.is_file())
        .filter(|path| !condition_report_is_stale(path))
        .collect::<Vec<_>>();
    report_paths.sort();
    let reports = report_paths
        .iter()
        .map(read_json_file::<DeNovoConditionReport>)
        .collect::<Result<Vec<_>>>()?;
    if reports.is_empty() {
        bail!(
            "no DeNovoSWE condition reports found under {}",
            conditions_dir.display()
        );
    }
    Ok(reports)
}

fn condition_report_is_stale(report_path: &Path) -> bool {
    let Some(condition_dir) = report_path.parent() else {
        return false;
    };
    let metadata_path = condition_dir.join("condition.json");
    let Ok(metadata) = read_json_file::<DeNovoConditionMetadata>(&metadata_path) else {
        return false;
    };
    let Some(results_jsonl) = metadata.results_jsonl else {
        return false;
    };
    let results_path = if results_jsonl.is_absolute() {
        results_jsonl
    } else {
        condition_dir.join(results_jsonl)
    };
    let Ok(report_modified) = fs::metadata(report_path).and_then(|metadata| metadata.modified())
    else {
        return false;
    };
    let Ok(results_modified) = fs::metadata(results_path).and_then(|metadata| metadata.modified())
    else {
        return false;
    };
    results_modified > report_modified
}

fn write_or_print(output: Option<&Path>, rendered: &str) -> Result<()> {
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(output, rendered)?;
    } else {
        println!("{rendered}");
    }
    Ok(())
}

fn axis_label(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

fn optional_float(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "n/a".to_string())
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
    pub subagent_used: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<DeNovoTokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_result: Option<DeNovoEvalResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeNovoTokenUsage {
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

impl DeNovoTokenUsage {
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
    #[serde(default)]
    pub subagent_observed_instances: usize,
    #[serde(default)]
    pub subagent_used_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_used_rate: Option<f64>,
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
    #[serde(default)]
    pub total_input_plus_output_tokens: u64,
    #[serde(default)]
    pub total_uncached_input_plus_output_tokens: u64,
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
    let subagent_observed_instances = results
        .iter()
        .filter(|result| result.subagent_used.is_some())
        .count();
    let subagent_used_count = results
        .iter()
        .filter(|result| result.subagent_used == Some(true))
        .count();
    let token_usages = results
        .iter()
        .filter_map(|result| result.token_usage.as_ref())
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
        subagent_observed_instances,
        subagent_used_count,
        subagent_used_rate: ratio(subagent_used_count, subagent_observed_instances),
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
        aweagent_commit,
    }
}

pub fn compare_denovo_reports(reports: Vec<DeNovoConditionReport>) -> DeNovoComparisonReport {
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
        total_input_plus_output_tokens,
        total_uncached_input_plus_output_tokens,
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
