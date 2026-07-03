pub mod denovo;
pub mod programbench;
mod shell_template;

pub use denovo::{
    DeNovoAgentDockerSandbox, DeNovoAgentKind, DeNovoCliRuntime, DeNovoCodexRunOptions,
    DeNovoCommand, DeNovoComparisonReport, DeNovoCondition, DeNovoConditionMetadata,
    DeNovoConditionReport, DeNovoConditionRunOptions, DeNovoEvalDetails, DeNovoEvalResult,
    DeNovoExtractMetadata, DeNovoExtractOptions, DeNovoExtractRecipeOptions,
    DeNovoMatrixRunOptions, DeNovoOfficialResult, DeNovoRunMode, DeNovoRunRecipeOptions,
    RecipeCommand, build_denovo_codex_adapter_command, build_denovo_condition_report,
    build_denovo_extract_recipe_command, build_denovo_run_recipe_command, compare_denovo_reports,
    default_denovo_conditions, parse_denovo_condition, render_denovo_comparison_markdown,
    render_denovo_report_markdown, run_denovo_condition, run_denovo_extract, run_denovo_matrix,
};

pub use programbench::{
    ProgramBenchAgentKind, ProgramBenchCommand, ProgramBenchComparisonReport,
    ProgramBenchCondition, ProgramBenchConditionMetadata, ProgramBenchConditionReport,
    ProgramBenchDiscoveredInstance, ProgramBenchEvalOptions, ProgramBenchInstanceMetadata,
    ProgramBenchInstanceReport, ProgramBenchInstanceRunOptions, ProgramBenchRecipeCommand,
    ProgramBenchRunOptions, ProgramBenchTokenUsage, build_programbench_agent_command,
    build_programbench_condition_report, build_programbench_eval_commands,
    compare_programbench_reports, default_programbench_conditions, parse_programbench_condition,
    planned_programbench_conditions, render_programbench_comparison_markdown,
    render_programbench_report_markdown, run_programbench_eval, run_programbench_matrix,
    run_programbench_matrix_with_instances,
};

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const DEFAULT_DATASET: &str = "SWE-bench/SWE-bench_Verified";
const DEFAULT_CONFIG: &str = "default";
const DEFAULT_SPLIT: &str = "test";
const DEFAULT_CACHE: &str = ".stateful_bench/datasets/swe-bench-verified.jsonl";
const DEFAULT_PAIRS: &str = ".stateful_bench/pairs/all.jsonl";
const DEFAULT_FALLBACK_PREFLIGHT: &str = ".stateful_bench/pairs/same-version-preflight.jsonl";
const DEFAULT_RUNS: &str = ".stateful_bench/runs";

#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGKILL: i32 = 9;

#[derive(Debug, Parser)]
#[command(name = "stateful-bench")]
#[command(about = "Concurrent editing benchmark runner for stateful coordination")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Fetch {
        #[arg(long, default_value = DEFAULT_DATASET)]
        dataset: String,
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: String,
        #[arg(long, default_value = DEFAULT_SPLIT)]
        split: String,
        #[arg(long, default_value = DEFAULT_CACHE)]
        output: PathBuf,
        #[arg(long, default_value_t = 100)]
        page_size: usize,
    },
    PreparePairs {
        #[arg(long, default_value = DEFAULT_CACHE)]
        dataset: PathBuf,
        #[arg(long, default_value = DEFAULT_PAIRS)]
        output: PathBuf,
        #[arg(long)]
        fallback_preflight: Option<PathBuf>,
    },
    GenerateFallbackPreflight {
        #[arg(long, default_value = DEFAULT_CACHE)]
        dataset: PathBuf,
        #[arg(long, default_value = DEFAULT_FALLBACK_PREFLIGHT)]
        output: PathBuf,
        #[arg(long)]
        assume_clean_apply: bool,
    },
    Sample {
        #[arg(long, default_value = DEFAULT_PAIRS)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 50)]
        count: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    Run {
        #[arg(long)]
        pairs: PathBuf,
        #[arg(long, value_enum)]
        mode: RunMode,
        #[arg(long, default_value_t = default_run_id())]
        run_id: String,
        #[arg(long)]
        agent_cmd_template: String,
        #[arg(long, default_value = DEFAULT_RUNS)]
        output_dir: PathBuf,
        #[arg(long, default_value_t = 1800)]
        timeout_seconds: u64,
        #[arg(long)]
        max_pairs: Option<usize>,
        #[arg(long = "pair-id")]
        pair_id: Vec<String>,
        #[arg(long, default_value_t = 1)]
        jobs: usize,
        #[arg(long)]
        auth_check_cmd_template: Option<String>,
        #[arg(long)]
        budget_check_cmd_template: Option<String>,
        #[arg(long)]
        setup_cmd_template: Option<String>,
        #[arg(long)]
        harness_cmd_template: Option<String>,
        #[arg(long, default_value = "stateful")]
        stateful_binary: String,
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
        stateful_run_dir: Vec<PathBuf>,
        #[arg(long, required = true)]
        no_state_run_dir: Vec<PathBuf>,
        #[arg(long)]
        awareness_run_dir: Vec<PathBuf>,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        max_pairs: Option<usize>,
        #[arg(long, value_enum, default_value_t = ReportFormat::Json)]
        format: ReportFormat,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Synthetic {
        #[arg(long, default_value = ".stateful_bench/synthetic")]
        output_dir: PathBuf,
        #[arg(long, default_value_t = default_synthetic_run_id())]
        run_id: String,
        #[arg(long, value_enum, default_value_t = ReportFormat::Json)]
        format: ReportFormat,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Denovo {
        #[command(subcommand)]
        command: denovo::DeNovoCommand,
    },
    Programbench {
        #[command(subcommand)]
        command: programbench::ProgramBenchCommand,
    },
}

pub fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Fetch {
            dataset,
            config,
            split,
            output,
            page_size,
        } => {
            let fetched = fetch_dataset(FetchConfig {
                dataset,
                config,
                split,
                output,
                page_size,
            })?;
            println!("{}", serde_json::json!({ "rows": fetched }));
        }
        Command::PreparePairs {
            dataset,
            output,
            fallback_preflight,
        } => {
            let instances = read_jsonl::<SweBenchInstance>(&dataset)?;
            let preflight = match fallback_preflight {
                Some(path) => Some(read_jsonl::<FallbackPreflight>(&path)?),
                None => None,
            };
            let prepared = prepare_pairs_from_instances(&instances, preflight.as_deref());
            write_jsonl(&output, &prepared.pairs)?;
            println!("{}", serde_json::to_string_pretty(&prepared.class_counts)?);
        }
        Command::GenerateFallbackPreflight {
            dataset,
            output,
            assume_clean_apply,
        } => {
            let instances = read_jsonl::<SweBenchInstance>(&dataset)?;
            let preflight =
                generate_fallback_preflight_from_instances(&instances, assume_clean_apply);
            let allowed = preflight.iter().filter(|record| record.allows()).count();
            write_jsonl(&output, &preflight)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "records": preflight.len(),
                    "allowed": allowed,
                    "assume_clean_apply": assume_clean_apply,
                }))?
            );
        }
        Command::Sample {
            input,
            output,
            count,
            seed,
        } => {
            let pairs = read_jsonl::<PairManifestEntry>(&input)?;
            let sample = stratified_sample(&pairs, count, seed);
            write_jsonl(&output, &sample)?;
            println!(
                "{}",
                serde_json::json!({ "pairs": sample.len(), "seed": seed })
            );
        }
        Command::Run {
            pairs,
            mode,
            run_id,
            agent_cmd_template,
            output_dir,
            timeout_seconds,
            max_pairs,
            pair_id,
            jobs,
            auth_check_cmd_template,
            budget_check_cmd_template,
            setup_cmd_template,
            harness_cmd_template,
            stateful_binary,
        } => {
            let output_dir_for_report = output_dir.clone();
            let metadata = run_pairs(RunOptions {
                pairs,
                mode,
                run_id,
                agent_cmd_template,
                output_dir,
                timeout_seconds,
                max_pairs,
                pair_ids: pair_id,
                jobs,
                auth_check_cmd_template,
                budget_check_cmd_template,
                setup_cmd_template,
                harness_cmd_template,
                stateful_binary,
            })?;
            println!("{}", serde_json::to_string_pretty(&metadata)?);
            let run_dir = absolute_path(&output_dir_for_report)?.join(&metadata.run_id);
            let report = build_report(&run_dir)?;
            ensure_run_has_scored_pairs(&report)?;
        }
        Command::Report {
            run_dir,
            format,
            output,
        } => {
            let report = build_report(&run_dir)?;
            let rendered = report.render(format)?;
            if let Some(output) = output {
                write_text_file(output, &rendered)?;
            } else {
                println!("{rendered}");
            }
        }
        Command::Compare {
            stateful_run_dir,
            no_state_run_dir,
            awareness_run_dir,
            manifest,
            max_pairs,
            format,
            output,
        } => {
            let report = compare_runs(CompareOptions {
                stateful_run_dir,
                no_state_run_dir,
                awareness_run_dir,
                manifest,
                max_pairs,
            })?;
            let rendered = report.render(format)?;
            if let Some(output) = output {
                write_text_file(output, &rendered)?;
            } else {
                println!("{rendered}");
            }
        }
        Command::Synthetic {
            output_dir,
            run_id,
            format,
            output,
        } => {
            let artifacts = run_synthetic_benchmark(SyntheticOptions { output_dir, run_id })?;
            let rendered = artifacts.comparison.render(format)?;
            if let Some(output) = output {
                write_text_file(output, &rendered)?;
            } else {
                println!("{rendered}");
            }
        }
        Command::Denovo { command } => {
            denovo::run_denovo_cli(command)?;
        }
        Command::Programbench { command } => {
            programbench::run_programbench_cli(command)?;
        }
    }

    Ok(())
}

fn default_run_id() -> String {
    format!("run-{}", uuid::Uuid::new_v4())
}

fn default_synthetic_run_id() -> String {
    format!("synthetic-{}", uuid::Uuid::new_v4())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum RunMode {
    Stateful,
    Awareness,
    NoState,
}

impl RunMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stateful => "stateful",
            Self::Awareness => "awareness",
            Self::NoState => "no-state",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ReportFormat {
    Json,
    Markdown,
}

impl std::fmt::Display for ReportFormat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json => formatter.write_str("json"),
            Self::Markdown => formatter.write_str("markdown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweBenchInstance {
    pub instance_id: String,
    pub repo: String,
    pub base_commit: String,
    #[serde(default)]
    pub problem_statement: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub patch: String,
    #[serde(default)]
    pub test_patch: String,
    #[serde(
        rename = "FAIL_TO_PASS",
        default,
        deserialize_with = "deserialize_test_list"
    )]
    pub fail_to_pass: Vec<String>,
    #[serde(
        rename = "PASS_TO_PASS",
        default,
        deserialize_with = "deserialize_test_list"
    )]
    pub pass_to_pass: Vec<String>,
    #[serde(default)]
    pub difficulty: Option<String>,
}

fn deserialize_test_list<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::String(text) => Ok(text),
                other => Err(serde::de::Error::custom(format!(
                    "expected string test id, got {other}"
                ))),
            })
            .collect(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Ok(Vec::new())
            } else if trimmed.starts_with('[') {
                serde_json::from_str::<Vec<String>>(trimmed).map_err(serde::de::Error::custom)
            } else {
                Ok(vec![text])
            }
        }
        other => Err(serde::de::Error::custom(format!(
            "expected string or array test list, got {other}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowsPage {
    pub instances: Vec<SweBenchInstance>,
    pub num_rows_total: usize,
    pub num_rows_per_page: usize,
    pub partial: bool,
}

#[derive(Debug, Deserialize)]
struct RawRowsPage {
    rows: Vec<RawRow>,
    num_rows_total: usize,
    #[serde(default)]
    num_rows_per_page: Option<usize>,
    #[serde(default)]
    partial: bool,
}

#[derive(Debug, Deserialize)]
struct RawRow {
    row: Value,
}

pub fn parse_rows_page(input: &str) -> Result<RowsPage> {
    let raw: RawRowsPage = serde_json::from_str(input)?;
    let instances = raw
        .rows
        .into_iter()
        .map(|row| serde_json::from_value(row.row).map_err(Into::into))
        .collect::<Result<Vec<SweBenchInstance>>>()?;

    Ok(RowsPage {
        instances,
        num_rows_total: raw.num_rows_total,
        num_rows_per_page: raw.num_rows_per_page.unwrap_or(100),
        partial: raw.partial,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchConfig {
    pub dataset: String,
    pub config: String,
    pub split: String,
    pub output: PathBuf,
    pub page_size: usize,
}

pub fn fetch_dataset(config: FetchConfig) -> Result<usize> {
    let page_size = config.page_size.clamp(1, 100);
    if let Some(parent) = config.output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(&config.output)
        .with_context(|| format!("failed to create {}", config.output.display()))?;
    let mut writer = BufWriter::new(file);
    let fetched = fetch_rows_to_writer(page_size, &mut writer, |offset, page_size| {
        let url = format!(
            "https://datasets-server.huggingface.co/rows?dataset={}&config={}&split={}&offset={offset}&length={page_size}",
            config.dataset, config.config, config.split
        );
        ureq::get(&url)
            .call()
            .with_context(|| format!("failed to fetch Hugging Face rows at offset {offset}"))?
            .into_string()
            .context("failed to read Hugging Face response body")
    })?;
    writer.flush()?;
    Ok(fetched)
}

pub fn fetch_rows_to_writer<W, F>(
    page_size: usize,
    mut writer: &mut W,
    mut fetch_page: F,
) -> Result<usize>
where
    W: Write,
    F: FnMut(usize, usize) -> Result<String>,
{
    let page_size = page_size.clamp(1, 100);
    let mut offset = 0usize;
    let mut fetched = 0usize;

    loop {
        let body = fetch_page(offset, page_size)?;
        let page = parse_rows_page(&body)?;
        let total = page.num_rows_total;

        if page.instances.is_empty() {
            break;
        }

        for instance in page.instances {
            serde_json::to_writer(&mut writer, &instance)?;
            writer.write_all(b"\n")?;
            fetched += 1;
        }

        offset = fetched;
        if fetched >= total {
            break;
        }
    }

    Ok(fetched)
}

pub fn read_jsonl<T>(path: impl AsRef<Path>) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    let path = path.as_ref();
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str(&line)
            .with_context(|| format!("failed to parse {} line {}", path.display(), index + 1))?;
        records.push(record);
    }
    Ok(records)
}

pub fn write_jsonl<T>(path: impl AsRef<Path>, records: &[T]) -> Result<()>
where
    T: Serialize,
{
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    for record in records {
        serde_json::to_writer(&mut writer, record)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

pub fn extract_touched_files(diff: &str) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            for token in rest.split_whitespace().take(2) {
                if let Some(path) = normalize_diff_path(token) {
                    files.insert(path);
                }
            }
            continue;
        }

        for prefix in ["--- ", "+++ ", "rename from ", "rename to "] {
            if let Some(path) = line.strip_prefix(prefix)
                && let Some(path) = normalize_diff_path(path)
            {
                files.insert(path);
            }
        }
    }
    files
}

fn normalize_diff_path(path: &str) -> Option<String> {
    let path = path
        .split('\t')
        .next()
        .unwrap_or(path)
        .trim()
        .trim_matches('"');
    if path == "/dev/null" || path.is_empty() {
        return None;
    }

    let stripped = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path);
    Some(stripped.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum PairClass {
    ExactFileOverlap,
    SameDirectory,
    SameRepoDisjoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairEligibility {
    SameBaseCommit,
    SameVersionPreflighted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairManifestEntry {
    pub pair_id: String,
    pub repo: String,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    pub eligibility: PairEligibility,
    pub class: PairClass,
    pub task_a_files: Vec<String>,
    pub task_b_files: Vec<String>,
    pub task_a: SweBenchInstance,
    pub task_b: SweBenchInstance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackPreflight {
    pub task_a: String,
    pub task_b: String,
    pub test_patch_clean_apply: bool,
    pub baseline_metadata_available: bool,
}

impl FallbackPreflight {
    fn allows_pair(&self, a: &str, b: &str) -> bool {
        self.allows()
            && ((self.task_a == a && self.task_b == b) || (self.task_a == b && self.task_b == a))
    }

    fn allows(&self) -> bool {
        self.test_patch_clean_apply && self.baseline_metadata_available
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPairs {
    pub pairs: Vec<PairManifestEntry>,
    pub class_counts: BTreeMap<PairClass, usize>,
}

pub fn classify_pair(a_files: &BTreeSet<String>, b_files: &BTreeSet<String>) -> PairClass {
    if a_files.iter().any(|file| b_files.contains(file)) {
        return PairClass::ExactFileOverlap;
    }

    if a_files.iter().any(|a| {
        b_files
            .iter()
            .any(|b| parent_directory(a).is_some_and(|parent| Some(parent) == parent_directory(b)))
    }) {
        return PairClass::SameDirectory;
    }

    PairClass::SameRepoDisjoint
}

fn parent_directory(path: &str) -> Option<&str> {
    let index = path.rfind('/')?;
    if index == 0 {
        None
    } else {
        Some(&path[..index])
    }
}

pub fn prepare_pairs_from_instances(
    instances: &[SweBenchInstance],
    fallback_preflight: Option<&[FallbackPreflight]>,
) -> PreparedPairs {
    let mut sorted = instances.to_vec();
    sorted.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));

    let mut pairs = Vec::new();
    let mut seen = BTreeSet::new();

    let mut exact_groups: BTreeMap<(&str, &str), Vec<&SweBenchInstance>> = BTreeMap::new();
    for instance in &sorted {
        exact_groups
            .entry((&instance.repo, &instance.base_commit))
            .or_default()
            .push(instance);
    }

    for group in exact_groups.values() {
        push_group_pairs(
            group,
            PairEligibility::SameBaseCommit,
            &mut seen,
            &mut pairs,
        );
    }

    if let Some(preflight) = fallback_preflight {
        let mut fallback_groups: BTreeMap<(&str, &str), Vec<&SweBenchInstance>> = BTreeMap::new();
        for instance in &sorted {
            if let Some(version) = instance.version.as_deref() {
                fallback_groups
                    .entry((&instance.repo, version))
                    .or_default()
                    .push(instance);
            }
        }

        for group in fallback_groups.values() {
            for first in 0..group.len() {
                for second in (first + 1)..group.len() {
                    let a = group[first];
                    let b = group[second];
                    let key = pair_key(&a.instance_id, &b.instance_id);
                    if seen.contains(&key) {
                        continue;
                    }
                    if preflight
                        .iter()
                        .any(|record| record.allows_pair(&a.instance_id, &b.instance_id))
                    {
                        seen.insert(key);
                        pairs.push(pair_entry(a, b, PairEligibility::SameVersionPreflighted));
                    }
                }
            }
        }
    }

    pairs.sort_by(|a, b| a.pair_id.cmp(&b.pair_id));
    let mut class_counts = BTreeMap::new();
    for pair in &pairs {
        *class_counts.entry(pair.class).or_insert(0) += 1;
    }

    PreparedPairs {
        pairs,
        class_counts,
    }
}

pub fn generate_fallback_preflight_from_instances(
    instances: &[SweBenchInstance],
    assume_clean_apply: bool,
) -> Vec<FallbackPreflight> {
    let mut sorted = instances.to_vec();
    sorted.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));

    let mut groups: BTreeMap<(&str, &str), Vec<&SweBenchInstance>> = BTreeMap::new();
    for instance in &sorted {
        if let Some(version) = instance.version.as_deref() {
            groups
                .entry((&instance.repo, version))
                .or_default()
                .push(instance);
        }
    }

    let mut records = Vec::new();
    for group in groups.values() {
        for first in 0..group.len() {
            for second in (first + 1)..group.len() {
                let a = group[first];
                let b = group[second];
                if a.base_commit == b.base_commit {
                    continue;
                }
                records.push(FallbackPreflight {
                    task_a: a.instance_id.clone(),
                    task_b: b.instance_id.clone(),
                    test_patch_clean_apply: assume_clean_apply
                        && has_test_patch(a)
                        && has_test_patch(b),
                    baseline_metadata_available: has_baseline_metadata(a)
                        && has_baseline_metadata(b),
                });
            }
        }
    }

    records
}

fn has_test_patch(instance: &SweBenchInstance) -> bool {
    !instance.test_patch.trim().is_empty()
}

fn has_baseline_metadata(instance: &SweBenchInstance) -> bool {
    !instance.fail_to_pass.is_empty()
}

fn push_group_pairs(
    group: &[&SweBenchInstance],
    eligibility: PairEligibility,
    seen: &mut BTreeSet<(String, String)>,
    pairs: &mut Vec<PairManifestEntry>,
) {
    for first in 0..group.len() {
        for second in (first + 1)..group.len() {
            let a = group[first];
            let b = group[second];
            let key = pair_key(&a.instance_id, &b.instance_id);
            if seen.insert(key) {
                pairs.push(pair_entry(a, b, eligibility));
            }
        }
    }
}

fn pair_entry(
    a: &SweBenchInstance,
    b: &SweBenchInstance,
    eligibility: PairEligibility,
) -> PairManifestEntry {
    let a_files = source_or_test_files(a);
    let b_files = source_or_test_files(b);
    let class = classify_pair(&a_files, &b_files);
    PairManifestEntry {
        pair_id: format!(
            "{}__{}",
            sanitize_id(&a.instance_id),
            sanitize_id(&b.instance_id)
        ),
        repo: a.repo.clone(),
        base_commit: match eligibility {
            PairEligibility::SameBaseCommit => Some(a.base_commit.clone()),
            PairEligibility::SameVersionPreflighted => None,
        },
        version: a.version.clone(),
        eligibility,
        class,
        task_a_files: a_files.into_iter().collect(),
        task_b_files: b_files.into_iter().collect(),
        task_a: a.clone(),
        task_b: b.clone(),
    }
}

fn source_or_test_files(instance: &SweBenchInstance) -> BTreeSet<String> {
    let source_files = extract_touched_files(&instance.patch);
    if source_files.is_empty() {
        extract_touched_files(&instance.test_patch)
    } else {
        source_files
    }
}

fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

pub fn stratified_sample(
    pairs: &[PairManifestEntry],
    count: usize,
    seed: u64,
) -> Vec<PairManifestEntry> {
    let count = count.min(pairs.len());
    if count == 0 {
        return Vec::new();
    }

    let class_order = [
        PairClass::ExactFileOverlap,
        PairClass::SameDirectory,
        PairClass::SameRepoDisjoint,
    ];
    let mut rng = StableRng::new(seed);
    let mut buckets: BTreeMap<PairClass, Vec<PairManifestEntry>> = BTreeMap::new();
    for pair in pairs {
        buckets.entry(pair.class).or_default().push(pair.clone());
    }
    for bucket in buckets.values_mut() {
        bucket.sort_by(|a, b| a.pair_id.cmp(&b.pair_id));
        rng.shuffle(bucket);
    }

    let present_classes = class_order
        .iter()
        .copied()
        .filter(|class| buckets.get(class).is_some_and(|bucket| !bucket.is_empty()))
        .collect::<Vec<_>>();
    let base = count / present_classes.len();
    let mut remainder = count % present_classes.len();
    let mut selected = Vec::new();
    let mut selected_ids = BTreeSet::new();

    for class in &present_classes {
        let target = base
            + if remainder > 0 {
                remainder -= 1;
                1
            } else {
                0
            };
        if let Some(bucket) = buckets.get(class) {
            for pair in bucket.iter().take(target) {
                selected_ids.insert(pair.pair_id.clone());
                selected.push(pair.clone());
            }
        }
    }

    if selected.len() < count {
        let mut remaining = pairs
            .iter()
            .filter(|pair| !selected_ids.contains(&pair.pair_id))
            .cloned()
            .collect::<Vec<_>>();
        remaining.sort_by(|a, b| a.pair_id.cmp(&b.pair_id));
        rng.shuffle(&mut remaining);
        selected.extend(remaining.into_iter().take(count - selected.len()));
    }

    selected.sort_by(|a, b| a.pair_id.cmp(&b.pair_id));
    selected
}

struct StableRng {
    state: u64,
}

impl StableRng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            let swap = (self.next_u64() as usize) % (index + 1);
            values.swap(index, swap);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessTaskOutcome {
    Passed,
    Failed,
    SetupError,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionalOutcome {
    pub task_a: HarnessTaskOutcome,
    pub task_b: HarnessTaskOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CollisionMetrics {
    pub uncoordinated_same_file_collisions: u64,
    pub coordinated_blocks: u64,
    pub lost_edit_events: u64,
    pub denied_writes: u64,
    pub scope_mismatches: u64,
    pub stale_intents: u64,
    pub timeouts: u64,
    pub long_idle_periods: u64,
    #[serde(default)]
    pub authorization_warnings: u64,
    #[serde(default)]
    pub warned_writes_applied: u64,
    #[serde(default)]
    pub wait_events: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositeScore {
    pub functional_pair_score: f64,
    pub collision_safety_score: f64,
    pub coordination_cost_score: f64,
    pub composite_coordination_score: f64,
}

pub fn composite_score(
    functional: &FunctionalOutcome,
    collisions: &CollisionMetrics,
) -> Option<CompositeScore> {
    composite_score_for_task_outcomes(&[functional.task_a, functional.task_b], collisions)
}

fn composite_score_for_task_outcomes(
    outcomes: &[HarnessTaskOutcome],
    collisions: &CollisionMetrics,
) -> Option<CompositeScore> {
    if outcomes.is_empty()
        || outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                HarnessTaskOutcome::SetupError | HarnessTaskOutcome::Unknown
            )
        })
    {
        return None;
    }

    let passed = outcomes
        .iter()
        .filter(|outcome| **outcome == HarnessTaskOutcome::Passed)
        .count();
    let functional_pair_score = passed as f64 / outcomes.len() as f64;

    let collision_penalty = collisions.uncoordinated_same_file_collisions as f64 * 0.3
        + collisions.lost_edit_events as f64 * 0.4;
    let cost_penalty = collisions.denied_writes as f64 * 0.1
        + collisions.scope_mismatches as f64 * 0.1
        + collisions.timeouts as f64 * 0.2
        + collisions.stale_intents as f64 * 0.05
        + collisions.long_idle_periods as f64 * 0.1;

    let collision_safety_score = round3((1.0 - collision_penalty).max(0.0));
    let coordination_cost_score = round3((1.0 - cost_penalty).max(0.0));
    let composite_coordination_score = round3(
        functional_pair_score * 0.5 + collision_safety_score * 0.3 + coordination_cost_score * 0.2,
    );

    Some(CompositeScore {
        functional_pair_score,
        collision_safety_score,
        coordination_cost_score,
        composite_coordination_score,
    })
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkEvidenceKind {
    #[default]
    PairedAgentRun,
    SyntheticFixture,
    Mixed,
}

impl std::fmt::Display for BenchmarkEvidenceKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::PairedAgentRun => "paired_agent_run",
            Self::SyntheticFixture => "synthetic_fixture",
            Self::Mixed => "mixed",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMetadata {
    pub run_id: String,
    pub mode: RunMode,
    #[serde(default)]
    pub evidence_kind: BenchmarkEvidenceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    pub pairs: PathBuf,
    pub mode: RunMode,
    pub run_id: String,
    pub agent_cmd_template: String,
    pub output_dir: PathBuf,
    pub timeout_seconds: u64,
    pub max_pairs: Option<usize>,
    pub pair_ids: Vec<String>,
    pub jobs: usize,
    pub auth_check_cmd_template: Option<String>,
    pub budget_check_cmd_template: Option<String>,
    pub setup_cmd_template: Option<String>,
    pub harness_cmd_template: Option<String>,
    pub stateful_binary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRunRecord {
    pub pair_id: String,
    pub mode: RunMode,
    pub agent_a: AgentRunRecord,
    pub agent_b: AgentRunRecord,
    #[serde(default)]
    pub agents: Vec<AgentRunRecord>,
    pub wall_time_ms: u64,
    pub combined_patch_path: String,
    #[serde(default)]
    pub harness_result_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunRecord {
    pub agent_id: String,
    pub outcome: AgentOutcome,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_token_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_token_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_token_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_output_token_count: Option<u64>,
}

impl AgentRunRecord {
    pub fn finished(agent_id: impl Into<String>, exit_code: i32, duration_ms: u64) -> Self {
        Self {
            agent_id: agent_id.into(),
            outcome: if exit_code == 0 {
                AgentOutcome::Succeeded
            } else {
                AgentOutcome::Failed
            },
            exit_code: Some(exit_code),
            duration_ms,
            token_count: None,
            input_token_count: None,
            cached_input_token_count: None,
            output_token_count: None,
            reasoning_output_token_count: None,
        }
    }

    fn timed_out(agent_id: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            agent_id: agent_id.into(),
            outcome: AgentOutcome::TimedOut,
            exit_code: None,
            duration_ms,
            token_count: None,
            input_token_count: None,
            cached_input_token_count: None,
            output_token_count: None,
            reasoning_output_token_count: None,
        }
    }

    fn failed(agent_id: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            agent_id: agent_id.into(),
            outcome: AgentOutcome::Failed,
            exit_code: None,
            duration_ms,
            token_count: None,
            input_token_count: None,
            cached_input_token_count: None,
            output_token_count: None,
            reasoning_output_token_count: None,
        }
    }

    fn with_usage_metrics(mut self, metrics: AgentUsageMetrics) -> Self {
        self.token_count = metrics.token_count;
        self.input_token_count = metrics.input_token_count;
        self.cached_input_token_count = metrics.cached_input_token_count;
        self.output_token_count = metrics.output_token_count;
        self.reasoning_output_token_count = metrics.reasoning_output_token_count;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AgentUsageMetrics {
    token_count: Option<u64>,
    input_token_count: Option<u64>,
    cached_input_token_count: Option<u64>,
    output_token_count: Option<u64>,
    reasoning_output_token_count: Option<u64>,
}

impl AgentUsageMetrics {
    fn add_agent(&mut self, agent: &AgentRunRecord) {
        self.token_count = add_optional(self.token_count, agent.token_count);
        self.input_token_count = add_optional(self.input_token_count, agent.input_token_count);
        self.cached_input_token_count = add_optional(
            self.cached_input_token_count,
            agent.cached_input_token_count,
        );
        self.output_token_count = add_optional(self.output_token_count, agent.output_token_count);
        self.reasoning_output_token_count = add_optional(
            self.reasoning_output_token_count,
            agent.reasoning_output_token_count,
        );
    }
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left + right),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcome {
    Succeeded,
    Failed,
    TimedOut,
}

pub fn run_pairs(options: RunOptions) -> Result<RunMetadata> {
    run_preflight_checks(&options)?;

    let mut pairs = read_jsonl::<PairManifestEntry>(&options.pairs)?;
    if !options.pair_ids.is_empty() {
        let selected = options
            .pair_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<String>>();
        pairs.retain(|pair| selected.contains(&pair.pair_id));
    }
    if let Some(max_pairs) = options.max_pairs {
        pairs.truncate(max_pairs);
    }

    let run_dir = absolute_path(&options.output_dir)?.join(&options.run_id);
    fs::create_dir_all(&run_dir)?;
    let metadata = RunMetadata {
        run_id: options.run_id.clone(),
        mode: options.mode,
        evidence_kind: BenchmarkEvidenceKind::PairedAgentRun,
    };
    write_json_file(run_dir.join("run.json"), &metadata)?;

    let jobs = options.jobs.max(1).min(pairs.len().max(1));
    if jobs == 1 {
        let abort = AtomicBool::new(false);
        for pair in &pairs {
            run_pair(pair, &options, &run_dir, &abort)?;
        }
    } else {
        let queue = Arc::new(Mutex::new(VecDeque::from(pairs)));
        let abort = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();
        for worker_index in 0..jobs {
            let queue = Arc::clone(&queue);
            let abort = Arc::clone(&abort);
            let options = options.clone();
            let run_dir = run_dir.clone();
            handles.push(thread::spawn(move || -> Result<()> {
                loop {
                    if abort.load(Ordering::SeqCst) {
                        return Ok(());
                    }

                    let pair = {
                        let mut queue = queue
                            .lock()
                            .map_err(|_| anyhow::anyhow!("pair queue lock poisoned"))?;
                        queue.pop_front()
                    };
                    let Some(pair) = pair else {
                        return Ok(());
                    };
                    if let Err(error) = run_pair(&pair, &options, &run_dir, &abort) {
                        abort.store(true, Ordering::SeqCst);
                        if is_fatal_agent_stop_error(&error) {
                            return Err(error);
                        }
                        return Err(error).with_context(|| {
                            format!("worker {worker_index} failed running pair {}", pair.pair_id)
                        });
                    }
                }
            }));
        }

        let mut first_error = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    abort.store(true, Ordering::SeqCst);
                    let replace = match first_error.as_ref() {
                        Some(current) => {
                            is_fatal_agent_abort_error(current)
                                && is_detected_fatal_agent_error(&error)
                        }
                        None => true,
                    };
                    if replace {
                        first_error = Some(error);
                    }
                }
                Err(_) => {
                    abort.store(true, Ordering::SeqCst);
                    if first_error.is_none() {
                        first_error = Some(anyhow::anyhow!("benchmark worker thread panicked"));
                    }
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
    }

    Ok(metadata)
}

fn run_preflight_checks(options: &RunOptions) -> Result<()> {
    if let Some(template) = &options.auth_check_cmd_template {
        run_shell_command(
            &render_run_template(template, options),
            Path::new("."),
            None,
            None,
            &[],
        )
        .context("auth preflight failed; benchmark aborted before spawning agents")?;
    }

    if let Some(template) = &options.budget_check_cmd_template {
        run_shell_command(
            &render_run_template(template, options),
            Path::new("."),
            None,
            None,
            &[],
        )
        .context("budget preflight failed; benchmark aborted before spawning agents")?;
    }

    Ok(())
}

fn run_pair(
    pair: &PairManifestEntry,
    options: &RunOptions,
    run_dir: &Path,
    abort: &AtomicBool,
) -> Result<()> {
    let started = Instant::now();
    if let Err(error) = run_pair_inner(pair, options, run_dir, abort) {
        if is_fatal_agent_stop_error(&error) {
            write_fatal_run_error(pair, run_dir, &format!("{error:#}"))?;
            return Err(error);
        }
        write_pair_error(
            pair,
            options,
            run_dir,
            elapsed_ms(started.elapsed()),
            &format!("{error:#}"),
        )?;
    }
    Ok(())
}

fn run_pair_inner(
    pair: &PairManifestEntry,
    options: &RunOptions,
    run_dir: &Path,
    abort: &AtomicBool,
) -> Result<()> {
    let pair_dir = run_dir.join(sanitize_id(&pair.pair_id));
    let workspace = pair_dir.join("workspace");
    let stateful_home = pair_dir.join("stateful-home");
    let stateful_env: Vec<(&str, &Path)> = if options.mode != RunMode::NoState {
        vec![("STATEFUL_HOME", stateful_home.as_path())]
    } else {
        Vec::new()
    };
    let stateful_workspace_id = format!("{}-{}", options.run_id, pair.pair_id);
    let agent_ids = agent_ids_for_pair(pair);
    fs::create_dir_all(&workspace)?;
    let pair_json = pair_dir.join("pair.json");
    write_json_file(&pair_json, pair)?;
    write_agent_task_artifacts(pair, &agent_ids, &pair_dir)?;

    if let Some(template) = &options.setup_cmd_template {
        run_shell_command(
            &render_template(
                template,
                &TemplateValues::for_pair(
                    pair,
                    &workspace,
                    &pair_json,
                    &pair_dir,
                    &stateful_workspace_id,
                    "setup",
                    "",
                ),
            ),
            &pair_dir,
            None,
            None,
            &stateful_env,
        )?;
    }

    write_agent_task_inputs(pair, &agent_ids, &workspace)?;

    let mut stateful_server = None;

    let result = (|| -> Result<()> {
        if options.mode != RunMode::NoState {
            stateful_server = Some(start_stateful_workspace(
                &workspace,
                &pair_dir,
                &options.stateful_binary,
                &stateful_workspace_id,
                &stateful_home,
                options.mode,
            )?);
        }

        let observer_path = pair_dir.join("observer-events.jsonl");
        let mut observer_events = vec![
            serde_json::json!({"event_type":"pair_started","pair_id":pair.pair_id,"mode":options.mode}),
        ];
        observer_events.extend(
            agent_ids.iter().map(
                |agent_id| serde_json::json!({"event_type":"agent_started","agent_id":agent_id}),
            ),
        );

        let timeout = Duration::from_secs(options.timeout_seconds);
        let started = Instant::now();
        let mut agents = Vec::new();
        for agent_id in &agent_ids {
            let log_stem = sanitize_id(agent_id);
            let stdout = pair_dir.join(format!("{log_stem}.stdout.log"));
            let stderr = pair_dir.join(format!("{log_stem}.stderr.log"));
            let child = spawn_agent(
                &options.agent_cmd_template,
                &TemplateValues::for_pair(
                    pair,
                    &workspace,
                    &pair_json,
                    &pair_dir,
                    &stateful_workspace_id,
                    agent_id,
                    &agent_task_suffix(agent_id),
                ),
                &workspace,
                &stdout,
                &stderr,
                &stateful_env,
            )?;
            agents.push(RunningAgent {
                agent_id: agent_id.clone(),
                child,
                stdout,
                stderr,
            });
        }

        let agent_records = wait_for_agents(&mut agents, timeout, abort)?;
        if let Some(error) = detect_agent_infrastructure_failure(&agents) {
            return Err(error.into());
        }
        let wall_time_ms = elapsed_ms(started.elapsed());
        if options.mode != RunMode::NoState {
            export_coordination_events(&stateful_home, &pair_dir)?;
        }

        observer_events.extend(agent_records.iter().map(|agent| {
            serde_json::json!({"event_type":"agent_finished","agent_id":agent.agent_id,"outcome":agent.outcome})
        }));
        if agent_records
            .iter()
            .any(|agent| agent.outcome == AgentOutcome::TimedOut)
        {
            observer_events.push(serde_json::json!({"event_type":"timeout"}));
        }
        observer_events
            .push(serde_json::json!({"event_type":"pair_finished","wall_time_ms":wall_time_ms}));
        write_jsonl(&observer_path, &observer_events)?;

        let combined_patch_path = pair_dir.join("combined.patch");
        write_combined_diff(&workspace, &combined_patch_path)?;

        let harness_result_path = if let Some(template) = &options.harness_cmd_template {
            let stdout = pair_dir.join("harness-result.json");
            let stderr = pair_dir.join("harness.stderr.log");
            run_shell_command(
                &shell_template::render(
                    &render_template(
                        template,
                        &TemplateValues::for_pair(
                            pair,
                            &workspace,
                            &pair_json,
                            &pair_dir,
                            &stateful_workspace_id,
                            "harness",
                            "",
                        ),
                    ),
                    &[(
                        "combined_patch",
                        combined_patch_path.to_string_lossy().into_owned(),
                    )],
                ),
                &workspace,
                Some(&stdout),
                Some(&stderr),
                &stateful_env,
            )?;
            Some("harness-result.json".to_string())
        } else {
            None
        };

        let agent_a = agent_records
            .first()
            .cloned()
            .unwrap_or_else(|| AgentRunRecord::failed("agent-a", wall_time_ms));
        let agent_b = agent_records
            .get(1)
            .cloned()
            .unwrap_or_else(|| AgentRunRecord::failed("agent-b", wall_time_ms));
        let record = PairRunRecord {
            pair_id: pair.pair_id.clone(),
            mode: options.mode,
            agent_a,
            agent_b,
            agents: agent_records,
            wall_time_ms,
            combined_patch_path: "combined.patch".to_string(),
            harness_result_path,
            error: None,
        };
        write_json_file(pair_dir.join("pair-run.json"), &record)?;

        Ok(())
    })();

    if let Some(child) = stateful_server.as_mut() {
        let _ = terminate_child(child);
    }

    result
}

fn write_pair_error(
    pair: &PairManifestEntry,
    options: &RunOptions,
    run_dir: &Path,
    wall_time_ms: u64,
    error: &str,
) -> Result<()> {
    let pair_dir = run_dir.join(sanitize_id(&pair.pair_id));
    let workspace = pair_dir.join("workspace");
    let agent_ids = agent_ids_for_pair(pair);
    fs::create_dir_all(&workspace)?;

    write_json_file(pair_dir.join("pair.json"), pair)?;
    write_agent_task_artifacts(pair, &agent_ids, &pair_dir)?;
    fs::write(pair_dir.join("run-error.txt"), error)?;

    let combined_patch_path = pair_dir.join("combined.patch");
    if !combined_patch_path.is_file() {
        fs::write(&combined_patch_path, [])?;
    }

    let harness_result = serde_json::json!({
        "task_results": agent_ids
            .iter()
            .map(|agent_id| serde_json::json!({"agent": agent_id, "status": "setup_error", "setup_error": true}))
            .collect::<Vec<_>>(),
        "error": error
    });
    write_json_file(pair_dir.join("harness-result.json"), &harness_result)?;

    let observer_path = pair_dir.join("observer-events.jsonl");
    let mut observer_events = if observer_path.is_file() {
        read_jsonl::<Value>(&observer_path)?
    } else {
        vec![serde_json::json!({
            "event_type": "pair_started",
            "pair_id": pair.pair_id,
            "mode": options.mode
        })]
    };
    observer_events.push(serde_json::json!({"event_type":"pair_error","error":error}));
    observer_events
        .push(serde_json::json!({"event_type":"pair_finished","wall_time_ms":wall_time_ms}));
    write_jsonl(&observer_path, &observer_events)?;

    let agent_records = agent_ids
        .iter()
        .map(|agent_id| AgentRunRecord::failed(agent_id.as_str(), wall_time_ms))
        .collect::<Vec<_>>();
    let record = PairRunRecord {
        pair_id: pair.pair_id.clone(),
        mode: options.mode,
        agent_a: agent_records
            .first()
            .cloned()
            .unwrap_or_else(|| AgentRunRecord::failed("agent-a", wall_time_ms)),
        agent_b: agent_records
            .get(1)
            .cloned()
            .unwrap_or_else(|| AgentRunRecord::failed("agent-b", wall_time_ms)),
        agents: agent_records,
        wall_time_ms,
        combined_patch_path: "combined.patch".to_string(),
        harness_result_path: Some("harness-result.json".to_string()),
        error: Some(error.to_string()),
    };
    write_json_file(pair_dir.join("pair-run.json"), &record)?;

    Ok(())
}

fn write_fatal_run_error(pair: &PairManifestEntry, run_dir: &Path, error: &str) -> Result<()> {
    let pair_dir = run_dir.join(sanitize_id(&pair.pair_id));
    fs::create_dir_all(&pair_dir)?;
    fs::write(pair_dir.join("run-error.txt"), error)?;
    fs::write(run_dir.join("fatal-error.txt"), error)?;
    Ok(())
}

fn write_agent_task_artifacts(
    pair: &PairManifestEntry,
    agent_ids: &[String],
    pair_dir: &Path,
) -> Result<()> {
    for agent_id in agent_ids {
        let suffix = agent_task_suffix(agent_id);
        write_json_file(
            pair_dir.join(format!("task-{suffix}.json")),
            instance_for_agent(pair, agent_id),
        )?;
    }
    Ok(())
}

fn write_agent_task_inputs(
    pair: &PairManifestEntry,
    agent_ids: &[String],
    workspace: &Path,
) -> Result<()> {
    let task_dir = workspace.join(".stateful_bench");
    fs::create_dir_all(&task_dir)?;
    for agent_id in agent_ids {
        let suffix = agent_task_suffix(agent_id);
        write_json_file(
            task_dir.join(format!("task-{suffix}.json")),
            &AgentTaskInput::from_instance(instance_for_agent(pair, agent_id)),
        )?;
    }
    Ok(())
}

fn instance_for_agent<'a>(pair: &'a PairManifestEntry, agent_id: &str) -> &'a SweBenchInstance {
    if agent_id == "agent-b" {
        &pair.task_b
    } else {
        &pair.task_a
    }
}

fn agent_ids_for_pair(pair: &PairManifestEntry) -> Vec<String> {
    let parsed = serde_json::from_str::<Value>(&pair.task_a.test_patch).ok();
    let mut agents = parsed
        .as_ref()
        .and_then(|metadata| metadata.get("agents"))
        .and_then(Value::as_array)
        .map(|items| {
            let mut seen = BTreeSet::new();
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|agent_id| !agent_id.is_empty())
                .filter(|agent_id| seen.insert((*agent_id).to_string()))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if agents.len() < 2 {
        agents = vec!["agent-a".to_string(), "agent-b".to_string()];
    }

    agents
}

fn agent_task_suffix(agent_id: &str) -> String {
    agent_id
        .strip_prefix("agent-")
        .unwrap_or(agent_id)
        .to_string()
}

#[derive(Debug, Serialize)]
struct AgentTaskInput<'a> {
    instance_id: &'a str,
    repo: &'a str,
    base_commit: &'a str,
    problem_statement: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    difficulty: Option<&'a str>,
}

impl<'a> AgentTaskInput<'a> {
    fn from_instance(instance: &'a SweBenchInstance) -> Self {
        Self {
            instance_id: &instance.instance_id,
            repo: &instance.repo,
            base_commit: &instance.base_commit,
            problem_statement: &instance.problem_statement,
            version: instance.version.as_deref(),
            difficulty: instance.difficulty.as_deref(),
        }
    }
}

fn spawn_agent(
    template: &str,
    values: &TemplateValues<'_>,
    workspace: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    extra_env: &[(&str, &Path)],
) -> Result<Child> {
    let command = render_template(template, values);
    let stdout = File::create(stdout_path)?;
    let stderr = File::create(stderr_path)?;
    let mut process = ProcessCommand::new("sh");
    process
        .arg("-c")
        .arg(command)
        .current_dir(workspace)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    apply_extra_env(&mut process, extra_env);
    isolate_process_group(&mut process);
    process.spawn().context("failed to spawn agent command")
}

struct RunningAgent {
    agent_id: String,
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
}

#[derive(Debug, Clone)]
struct AgentLogPaths {
    agent_id: String,
    stdout: PathBuf,
    stderr: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FatalAgentFailureKind {
    UsageLimit,
    PlatformFailure,
}

#[derive(Debug, Clone)]
struct FatalAgentFailureError {
    kind: FatalAgentFailureKind,
    agent_id: String,
    log_path: PathBuf,
    excerpt: String,
}

impl fmt::Display for FatalAgentFailureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            FatalAgentFailureKind::UsageLimit => write!(
                formatter,
                "agent usage limit detected for {} in {}: {}",
                self.agent_id,
                self.log_path.display(),
                self.excerpt
            ),
            FatalAgentFailureKind::PlatformFailure => write!(
                formatter,
                "agent platform failure detected for {} in {}: {}",
                self.agent_id,
                self.log_path.display(),
                self.excerpt
            ),
        }
    }
}

impl std::error::Error for FatalAgentFailureError {}

#[derive(Debug, Clone)]
struct FatalAgentAbortError;

impl fmt::Display for FatalAgentAbortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "agent run aborted because another worker detected a fatal agent failure"
        )
    }
}

impl std::error::Error for FatalAgentAbortError {}

#[derive(Debug, Clone)]
struct AgentInfrastructureFailureError {
    agent_id: String,
    log_path: PathBuf,
    excerpt: String,
}

impl fmt::Display for AgentInfrastructureFailureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "agent infrastructure failure detected for {} in {}: {}",
            self.agent_id,
            self.log_path.display(),
            self.excerpt
        )
    }
}

impl std::error::Error for AgentInfrastructureFailureError {}

fn is_fatal_agent_stop_error(error: &anyhow::Error) -> bool {
    is_detected_fatal_agent_error(error) || is_fatal_agent_abort_error(error)
}

fn is_detected_fatal_agent_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<FatalAgentFailureError>().is_some()
}

fn is_fatal_agent_abort_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<FatalAgentAbortError>().is_some()
}

fn wait_for_agents(
    agents: &mut [RunningAgent],
    timeout: Duration,
    abort: &AtomicBool,
) -> Result<Vec<AgentRunRecord>> {
    let started = Instant::now();
    let mut last_fatal_check = Instant::now();
    let logs = agents
        .iter()
        .map(|agent| AgentLogPaths {
            agent_id: agent.agent_id.clone(),
            stdout: agent.stdout.clone(),
            stderr: agent.stderr.clone(),
        })
        .collect::<Vec<_>>();
    let mut records = vec![None; agents.len()];

    while records.iter().any(Option::is_none) {
        if abort.load(Ordering::SeqCst) {
            terminate_agent_children(agents);
            return Err(FatalAgentAbortError.into());
        }

        if last_fatal_check.elapsed() >= Duration::from_millis(250) {
            if let Some(error) = detect_fatal_agent_failure(&logs) {
                abort.store(true, Ordering::SeqCst);
                terminate_agent_children(agents);
                return Err(error.into());
            }
            last_fatal_check = Instant::now();
        }

        for (index, agent) in agents.iter_mut().enumerate() {
            if records[index].is_none()
                && let Ok(Some(status)) = agent.child.try_wait()
            {
                records[index] = Some(
                    AgentRunRecord::finished(
                        agent.agent_id.as_str(),
                        status.code().unwrap_or(1),
                        elapsed_ms(started.elapsed()),
                    )
                    .with_usage_metrics(read_agent_usage_metrics(&agent.stdout)),
                );
            }
        }

        if started.elapsed() >= timeout {
            for (index, agent) in agents.iter_mut().enumerate() {
                if records[index].is_none() {
                    let _ = terminate_child(&mut agent.child);
                    records[index] = Some(
                        AgentRunRecord::timed_out(
                            agent.agent_id.as_str(),
                            elapsed_ms(started.elapsed()),
                        )
                        .with_usage_metrics(read_agent_usage_metrics(&agent.stdout)),
                    );
                }
            }
        }

        if records.iter().any(Option::is_none) {
            thread::sleep(Duration::from_millis(25));
        }
    }

    if let Some(error) = detect_fatal_agent_failure(&logs) {
        abort.store(true, Ordering::SeqCst);
        return Err(error.into());
    }

    Ok(records
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            record.unwrap_or_else(|| {
                AgentRunRecord::timed_out(
                    agents[index].agent_id.as_str(),
                    elapsed_ms(started.elapsed()),
                )
                .with_usage_metrics(read_agent_usage_metrics(&agents[index].stdout))
            })
        })
        .collect())
}

fn read_agent_usage_metrics(path: &Path) -> AgentUsageMetrics {
    let Ok(file) = File::open(path) else {
        return AgentUsageMetrics::default();
    };
    let mut metrics = AgentUsageMetrics::default();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(usage) = extract_codex_usage_metrics(&value) {
            metrics = usage;
        }
    }
    metrics
}

fn extract_codex_usage_metrics(value: &Value) -> Option<AgentUsageMetrics> {
    [
        value.pointer("/info/total_token_usage"),
        value.pointer("/payload/info/total_token_usage"),
        value.pointer("/usage"),
        value.pointer("/payload/usage"),
        value.pointer("/response/usage"),
        value.pointer("/payload/response/usage"),
    ]
    .into_iter()
    .flatten()
    .find_map(usage_metrics_from_value)
}

fn usage_metrics_from_value(value: &Value) -> Option<AgentUsageMetrics> {
    let input_token_count = value.get("input_tokens").and_then(Value::as_u64);
    let cached_input_token_count = value
        .get("cached_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
        });
    let output_token_count = value.get("output_tokens").and_then(Value::as_u64);
    let reasoning_output_token_count = value
        .get("reasoning_output_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .pointer("/output_tokens_details/reasoning_tokens")
                .and_then(Value::as_u64)
        });
    let token_count = value
        .get("total_tokens")
        .and_then(Value::as_u64)
        .or_else(|| value.get("token_count").and_then(Value::as_u64))
        .or_else(|| add_optional(input_token_count, output_token_count));

    if token_count.is_none()
        && input_token_count.is_none()
        && cached_input_token_count.is_none()
        && output_token_count.is_none()
        && reasoning_output_token_count.is_none()
    {
        return None;
    }

    Some(AgentUsageMetrics {
        token_count,
        input_token_count,
        cached_input_token_count,
        output_token_count,
        reasoning_output_token_count,
    })
}

fn terminate_agent_children(agents: &mut [RunningAgent]) {
    for agent in agents {
        let _ = terminate_child(&mut agent.child);
    }
}

fn detect_fatal_agent_failure(logs: &[AgentLogPaths]) -> Option<FatalAgentFailureError> {
    logs.iter().find_map(|log| {
        [&log.stdout, &log.stderr].into_iter().find_map(|path| {
            fatal_agent_failure_excerpt(path).map(|(kind, excerpt)| FatalAgentFailureError {
                kind,
                agent_id: log.agent_id.clone(),
                log_path: path.to_path_buf(),
                excerpt,
            })
        })
    })
}

fn detect_agent_infrastructure_failure(
    agents: &[RunningAgent],
) -> Option<AgentInfrastructureFailureError> {
    agents.iter().find_map(|agent| {
        [&agent.stdout, &agent.stderr].into_iter().find_map(|path| {
            agent_infrastructure_failure_excerpt(path).map(|excerpt| {
                AgentInfrastructureFailureError {
                    agent_id: agent.agent_id.clone(),
                    log_path: path.to_path_buf(),
                    excerpt,
                }
            })
        })
    })
}

fn agent_infrastructure_failure_excerpt(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .find(|line| contains_agent_infrastructure_failure_text(line))
        .map(trim_log_excerpt)
}

fn contains_agent_infrastructure_failure_text(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    [
        "failed to initialize in-process app-server client",
        "sandbox_apply: operation not permitted",
        "user cancelled mcp tool call",
        "i couldn't complete the edit",
        "i could not complete the edit",
    ]
    .iter()
    .any(|pattern| line.contains(pattern))
}

fn fatal_agent_failure_excerpt(path: &Path) -> Option<(FatalAgentFailureKind, String)> {
    let bytes = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .find_map(|line| classify_fatal_agent_line(line).map(|kind| (kind, trim_log_excerpt(line))))
}

fn classify_fatal_agent_line(line: &str) -> Option<FatalAgentFailureKind> {
    if contains_fatal_agent_limit_text(line) {
        return Some(FatalAgentFailureKind::UsageLimit);
    }
    if contains_codex_platform_failure_event(line) {
        return Some(FatalAgentFailureKind::PlatformFailure);
    }
    None
}

fn contains_fatal_agent_limit_text(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    [
        "you've hit your usage limit",
        "you have hit your usage limit",
        "usage limit",
        "insufficient_quota",
        "rate limit",
        "rate_limit",
        "purchase more credits",
    ]
    .iter()
    .any(|pattern| line.contains(pattern))
}

fn contains_codex_platform_failure_event(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("turn.failed" | "error")
    )
}

fn trim_log_excerpt(line: &str) -> String {
    let line = line.trim();
    const MAX_CHARS: usize = 300;
    if line.chars().count() <= MAX_CHARS {
        return line.to_string();
    }

    let mut trimmed = line.chars().take(MAX_CHARS).collect::<String>();
    trimmed.push_str("...");
    trimmed
}

fn start_stateful_workspace(
    workspace: &Path,
    pair_dir: &Path,
    stateful_binary: &str,
    workspace_id: &str,
    stateful_home: &Path,
    mode: RunMode,
) -> Result<Child> {
    run_shell_command(
        &format!("{stateful_binary} enable"),
        workspace,
        None,
        None,
        &[("STATEFUL_HOME", stateful_home)],
    )
    .context("failed to enable stateful hooks in benchmark workspace")?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);

    let stdout = File::create(pair_dir.join("stateful-server.stdout.log"))?;
    let stderr = File::create(pair_dir.join("stateful-server.stderr.log"))?;
    let mut process = ProcessCommand::new(stateful_binary);
    process
        .args(stateful_server_args(mode, port, workspace_id))
        .current_dir(workspace)
        .env("STATEFUL_HOME", stateful_home)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    isolate_process_group(&mut process);
    let mut child = process
        .spawn()
        .context("failed to start stateful server in benchmark workspace")?;
    wait_for_stateful_ready(stateful_home, &mut child, Duration::from_secs(10)).with_context(
        || {
            format!(
                "stateful server did not become ready; see {} and {}",
                pair_dir.join("stateful-server.stdout.log").display(),
                pair_dir.join("stateful-server.stderr.log").display()
            )
        },
    )?;
    Ok(child)
}

fn stateful_server_args(mode: RunMode, port: u16, workspace_id: &str) -> Vec<String> {
    let mut args = vec![
        "server".to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--workspace-id".to_string(),
        workspace_id.to_string(),
    ];
    if mode == RunMode::Awareness {
        args.extend(["--coordination-mode".to_string(), "awareness".to_string()]);
    }
    args
}

fn export_coordination_events(stateful_home: &Path, pair_dir: &Path) -> Result<()> {
    let runtime: Value = read_json_file(stateful_home.join("runtime/server.json"))?;
    let base_url = runtime
        .get("base_url")
        .and_then(Value::as_str)
        .context("stateful runtime missing base_url")?;
    let token = runtime
        .get("token")
        .and_then(Value::as_str)
        .context("stateful runtime missing token")?;
    let url = format!("{}/v1/events", base_url.trim_end_matches('/'));
    let body = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .context("failed to export stateful coordination events")?
        .into_string()
        .context("failed to read stateful coordination events")?;
    let value: Value = serde_json::from_str(&body).context("failed to parse stateful events")?;
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    write_jsonl(pair_dir.join("coordination-events.jsonl"), &events)
}

fn wait_for_stateful_ready(
    stateful_home: &Path,
    child: &mut Child,
    timeout: Duration,
) -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("stateful server exited before readiness with status {status}");
        }
        if stateful_current_is_ready(stateful_home) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }

    anyhow::bail!("timed out waiting for stateful server readiness")
}

fn stateful_current_is_ready(stateful_home: &Path) -> bool {
    let runtime_path = stateful_home.join("runtime/server.json");
    let Ok(runtime) = read_json_file::<Value>(&runtime_path) else {
        return false;
    };
    let Some(base_url) = runtime.get("base_url").and_then(Value::as_str) else {
        return false;
    };
    let Some(token) = runtime.get("token").and_then(Value::as_str) else {
        return false;
    };
    let url = format!("{}/v1/current", base_url.trim_end_matches('/'));
    ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .is_ok_and(|response| (200..300).contains(&response.status()))
}

fn terminate_child(child: &mut Child) -> Result<()> {
    #[cfg(unix)]
    {
        signal_process_group(child, SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let exited_after_term = wait_for_child_exit(child, Duration::from_millis(500))?;

    #[cfg(unix)]
    signal_process_group(child, SIGKILL);
    if !exited_after_term {
        let _ = child.kill();
    }
    let _ = wait_for_child_exit(child, Duration::from_millis(500))?;
    Ok(())
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> Result<bool> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(false)
}

fn isolate_process_group(process: &mut ProcessCommand) {
    #[cfg(unix)]
    {
        process.process_group(0);
    }
}

#[cfg(unix)]
fn signal_process_group(child: &Child, signal: i32) {
    let signal = match signal {
        SIGTERM => "-TERM",
        SIGKILL => "-KILL",
        _ => return,
    };
    let group = format!("-{}", child.id());
    let _ = ProcessCommand::new("/bin/kill")
        .args([signal, group.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn run_shell_command(
    command: &str,
    cwd: &Path,
    stdout_path: Option<&Path>,
    stderr_path: Option<&Path>,
    extra_env: &[(&str, &Path)],
) -> Result<()> {
    let mut process = ProcessCommand::new("sh");
    process.arg("-c").arg(command).current_dir(cwd);
    apply_extra_env(&mut process, extra_env);
    if let Some(path) = stdout_path {
        process.stdout(Stdio::from(File::create(path)?));
    }
    if let Some(path) = stderr_path {
        process.stderr(Stdio::from(File::create(path)?));
    }
    let status = process.status()?;
    if !status.success() {
        anyhow::bail!("command failed with status {status}: {command}");
    }
    Ok(())
}

fn apply_extra_env(process: &mut ProcessCommand, extra_env: &[(&str, &Path)]) {
    for (key, value) in extra_env {
        process.env(key, value);
    }
}

fn write_combined_diff(workspace: &Path, output_path: &Path) -> Result<()> {
    let output = ProcessCommand::new("git")
        .args(["diff", "--binary"])
        .current_dir(workspace)
        .output();
    match output {
        Ok(output) if output.status.success() => fs::write(output_path, output.stdout)?,
        Ok(output) => fs::write(output_path, output.stderr)?,
        Err(error) => fs::write(output_path, format!("git diff unavailable: {error}\n"))?,
    }
    Ok(())
}

struct TemplateValues<'a> {
    workspace: &'a Path,
    pair_json: &'a Path,
    run_dir: &'a Path,
    task_json: PathBuf,
    session_id: String,
    stateful_workspace_id: String,
    agent_id: &'a str,
    pair_id: &'a str,
    repo: &'a str,
    base_commit: &'a str,
}

impl<'a> TemplateValues<'a> {
    fn for_pair(
        pair: &'a PairManifestEntry,
        workspace: &'a Path,
        pair_json: &'a Path,
        run_dir: &'a Path,
        stateful_workspace_id: &str,
        agent_id: &'a str,
        task_suffix: &str,
    ) -> Self {
        let task_json = if task_suffix.is_empty() {
            pair_json.to_path_buf()
        } else {
            workspace.join(format!(".stateful_bench/task-{task_suffix}.json"))
        };
        Self {
            workspace,
            pair_json,
            run_dir,
            task_json,
            session_id: format!("{}-{agent_id}", pair.pair_id),
            stateful_workspace_id: stateful_workspace_id.to_string(),
            agent_id,
            pair_id: &pair.pair_id,
            repo: &pair.repo,
            base_commit: pair
                .base_commit
                .as_deref()
                .unwrap_or(&pair.task_a.base_commit),
        }
    }
}

fn render_template(template: &str, values: &TemplateValues<'_>) -> String {
    shell_template::render(
        template,
        &[
            ("workspace", values.workspace.to_string_lossy().into_owned()),
            ("task_json", values.task_json.to_string_lossy().into_owned()),
            ("pair_json", values.pair_json.to_string_lossy().into_owned()),
            ("run_dir", values.run_dir.to_string_lossy().into_owned()),
            ("session_id", values.session_id.clone()),
            (
                "stateful_workspace_id",
                values.stateful_workspace_id.clone(),
            ),
            ("agent_id", values.agent_id.to_string()),
            ("pair_id", values.pair_id.to_string()),
            ("repo", values.repo.to_string()),
            ("base_commit", values.base_commit.to_string()),
        ],
    )
}

fn render_run_template(template: &str, options: &RunOptions) -> String {
    shell_template::render(
        template,
        &[
            ("run_id", options.run_id.clone()),
            ("mode", options.mode.as_str().to_string()),
            (
                "output_dir",
                options.output_dir.to_string_lossy().into_owned(),
            ),
        ],
    )
}

fn absolute_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn elapsed_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    pub run_id: String,
    pub mode: RunMode,
    pub evidence_kind: BenchmarkEvidenceKind,
    pub summary: ReportSummary,
    pub pairs: Vec<PairReport>,
}

impl RunReport {
    pub fn render(&self, format: ReportFormat) -> Result<String> {
        match format {
            ReportFormat::Json => Ok(serde_json::to_string_pretty(self)?),
            ReportFormat::Markdown => Ok(render_report_markdown(self)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareOptions {
    pub stateful_run_dir: Vec<PathBuf>,
    pub no_state_run_dir: Vec<PathBuf>,
    pub awareness_run_dir: Vec<PathBuf>,
    pub manifest: PathBuf,
    pub max_pairs: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub stateful_run_id: String,
    pub no_state_run_id: String,
    pub evidence_kind: BenchmarkEvidenceKind,
    pub empirical_claim_allowed: bool,
    pub evidence_notes: Vec<String>,
    pub manifest_path: String,
    pub manifest_pairs: usize,
    pub paired: PairedComparisonSummary,
    pub stateful: ModeComparisonSummary,
    pub no_state: ModeComparisonSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awareness: Option<ModeComparisonSummary>,
    pub coordination_effects: CoordinationEffectSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_vs_awareness: Option<CoordinationEffectSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awareness_vs_enforcement: Option<CoordinationEffectSummary>,
    pub pairs: Vec<PairComparison>,
    pub excluded_pairs: Vec<ExcludedComparisonPair>,
}

impl ComparisonReport {
    pub fn render(&self, format: ReportFormat) -> Result<String> {
        match format {
            ReportFormat::Json => Ok(serde_json::to_string_pretty(self)?),
            ReportFormat::Markdown => Ok(self.render_markdown()),
        }
    }

    pub fn render_markdown(&self) -> String {
        render_comparison_markdown(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairedComparisonSummary {
    pub paired_valid_pairs: usize,
    pub stateful_functional_score: Option<f64>,
    pub no_state_functional_score: Option<f64>,
    pub paired_valid_functional_delta: Option<f64>,
    pub raw_manifest_functional_delta: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationEffectSummary {
    pub prevented_uncoordinated_same_file_collisions: i64,
    pub prevented_lost_edit_events: i64,
    pub additional_coordinated_blocks: i64,
    pub additional_denied_writes: i64,
    pub additional_scope_mismatches: i64,
    pub additional_stale_intents: i64,
    pub additional_timeouts: i64,
    pub additional_long_idle_periods: i64,
    pub additional_false_blocks: i64,
    pub additional_manual_interventions: i64,
    pub additional_coordination_friction_events: i64,
    pub additional_wall_time_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeComparisonSummary {
    pub run_id: String,
    pub artifact_pairs: usize,
    pub missing_artifacts: usize,
    pub scored_pairs: usize,
    pub setup_error_pairs: usize,
    pub unknown_pairs: usize,
    pub available_valid_score: Option<f64>,
    pub raw_manifest_score: f64,
    pub infra_loss_rate: f64,
    pub task_passed: usize,
    pub task_failed: usize,
    pub setup_errors: usize,
    pub unknown_task_results: usize,
    pub uncoordinated_same_file_collisions: u64,
    pub coordinated_blocks: u64,
    pub lost_edit_events: u64,
    pub denied_writes: u64,
    pub scope_mismatches: u64,
    pub stale_intents: u64,
    pub timeouts: u64,
    pub long_idle_periods: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub authorization_warnings: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub warned_writes_applied: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub wait_events: u64,
    pub wall_time_ms: u64,
    pub token_count: Option<u64>,
    pub tool_call_count: Option<u64>,
    pub preserved_edit_count: u64,
    pub missing_expected_line_count: u64,
    pub false_block_count: u64,
    pub missed_conflict_count: u64,
    pub manual_intervention_count: u64,
    pub time_to_converge_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairComparison {
    pub pair_id: String,
    pub stateful_status: PairComparisonStatus,
    pub no_state_status: PairComparisonStatus,
    pub stateful_functional_pair_score: Option<f64>,
    pub no_state_functional_pair_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awareness_status: Option<PairComparisonStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awareness_functional_pair_score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExcludedComparisonPair {
    pub pair_id: String,
    pub stateful_status: PairComparisonStatus,
    pub no_state_status: PairComparisonStatus,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairComparisonStatus {
    Scored,
    MissingArtifact,
    SetupError,
    Unknown,
}

pub fn compare_runs(options: CompareOptions) -> Result<ComparisonReport> {
    let mut manifest = read_jsonl::<PairManifestEntry>(&options.manifest)?;
    if let Some(max_pairs) = options.max_pairs {
        manifest.truncate(max_pairs);
    }
    let pair_ids = manifest
        .iter()
        .map(|pair| pair.pair_id.clone())
        .collect::<Vec<_>>();
    let stateful_reports = build_reports(&options.stateful_run_dir)?;
    let no_state_reports = build_reports(&options.no_state_run_dir)?;
    let awareness_reports = if options.awareness_run_dir.is_empty() {
        Vec::new()
    } else {
        build_reports(&options.awareness_run_dir)?
    };
    let stateful_run_id = joined_run_ids(&stateful_reports);
    let no_state_run_id = joined_run_ids(&no_state_reports);
    let awareness_run_id =
        (!awareness_reports.is_empty()).then(|| joined_run_ids(&awareness_reports));
    let evidence_kind = comparison_evidence_kind(
        stateful_reports
            .iter()
            .chain(no_state_reports.iter())
            .chain(awareness_reports.iter())
            .map(|report| report.evidence_kind),
    );
    let stateful_by_pair = pair_report_map(&stateful_reports);
    let no_state_by_pair = pair_report_map(&no_state_reports);
    let awareness_by_pair = pair_report_map(&awareness_reports);

    let stateful =
        summarize_mode_for_manifest(stateful_run_id.clone(), &pair_ids, &stateful_by_pair);
    let no_state =
        summarize_mode_for_manifest(no_state_run_id.clone(), &pair_ids, &no_state_by_pair);
    let awareness = awareness_run_id
        .clone()
        .map(|run_id| summarize_mode_for_manifest(run_id, &pair_ids, &awareness_by_pair));

    let mut pairs = Vec::new();
    let mut excluded_pairs = Vec::new();
    let mut paired_stateful_score = 0.0;
    let mut paired_no_state_score = 0.0;
    let mut paired_valid_pairs = 0usize;

    for pair_id in &pair_ids {
        let stateful_pair = stateful_by_pair.get(pair_id);
        let no_state_pair = no_state_by_pair.get(pair_id);
        let awareness_pair = awareness
            .as_ref()
            .and_then(|_| awareness_by_pair.get(pair_id));
        let stateful_status = comparison_status(stateful_pair);
        let no_state_status = comparison_status(no_state_pair);
        let awareness_status = awareness_pair.map(|pair| comparison_status(Some(pair)));
        let stateful_score = functional_pair_score(stateful_pair);
        let no_state_score = functional_pair_score(no_state_pair);
        let awareness_score = functional_pair_score(awareness_pair);

        if stateful_status == PairComparisonStatus::Scored
            && no_state_status == PairComparisonStatus::Scored
        {
            paired_valid_pairs += 1;
            paired_stateful_score += stateful_score.unwrap_or(0.0);
            paired_no_state_score += no_state_score.unwrap_or(0.0);
        } else {
            excluded_pairs.push(ExcludedComparisonPair {
                pair_id: pair_id.clone(),
                stateful_status,
                no_state_status,
                reason: exclusion_reason(stateful_status, no_state_status).to_string(),
            });
        }

        pairs.push(PairComparison {
            pair_id: pair_id.clone(),
            stateful_status,
            no_state_status,
            stateful_functional_pair_score: stateful_score,
            no_state_functional_pair_score: no_state_score,
            awareness_status,
            awareness_functional_pair_score: awareness_score,
        });
    }

    let stateful_paired = average_score(paired_stateful_score, paired_valid_pairs);
    let no_state_paired = average_score(paired_no_state_score, paired_valid_pairs);
    let paired_delta = match (stateful_paired, no_state_paired) {
        (Some(stateful), Some(no_state)) => Some(round3(stateful - no_state)),
        _ => None,
    };
    let raw_delta = (!pair_ids.is_empty()).then_some(round3(
        stateful.raw_manifest_score - no_state.raw_manifest_score,
    ));
    let coordination_effects = summarize_coordination_effects(&stateful, &no_state);
    let off_vs_awareness = awareness
        .as_ref()
        .map(|awareness| summarize_coordination_effects(awareness, &no_state));
    let awareness_vs_enforcement = awareness
        .as_ref()
        .map(|awareness| summarize_coordination_effects(&stateful, awareness));

    Ok(ComparisonReport {
        stateful_run_id,
        no_state_run_id,
        evidence_kind,
        empirical_claim_allowed: empirical_claim_allowed(evidence_kind),
        evidence_notes: evidence_notes(evidence_kind),
        manifest_path: options.manifest.to_string_lossy().into_owned(),
        manifest_pairs: pair_ids.len(),
        paired: PairedComparisonSummary {
            paired_valid_pairs,
            stateful_functional_score: stateful_paired,
            no_state_functional_score: no_state_paired,
            paired_valid_functional_delta: paired_delta,
            raw_manifest_functional_delta: raw_delta,
        },
        stateful,
        no_state,
        awareness,
        coordination_effects,
        off_vs_awareness,
        awareness_vs_enforcement,
        pairs,
        excluded_pairs,
    })
}

fn summarize_coordination_effects(
    stateful: &ModeComparisonSummary,
    no_state: &ModeComparisonSummary,
) -> CoordinationEffectSummary {
    CoordinationEffectSummary {
        prevented_uncoordinated_same_file_collisions: signed_delta(
            no_state.uncoordinated_same_file_collisions,
            stateful.uncoordinated_same_file_collisions,
        ),
        prevented_lost_edit_events: signed_delta(
            no_state.lost_edit_events,
            stateful.lost_edit_events,
        ),
        additional_coordinated_blocks: signed_delta(
            stateful.coordinated_blocks,
            no_state.coordinated_blocks,
        ),
        additional_denied_writes: signed_delta(stateful.denied_writes, no_state.denied_writes),
        additional_scope_mismatches: signed_delta(
            stateful.scope_mismatches,
            no_state.scope_mismatches,
        ),
        additional_stale_intents: signed_delta(stateful.stale_intents, no_state.stale_intents),
        additional_timeouts: signed_delta(stateful.timeouts, no_state.timeouts),
        additional_long_idle_periods: signed_delta(
            stateful.long_idle_periods,
            no_state.long_idle_periods,
        ),
        additional_false_blocks: signed_delta(
            stateful.false_block_count,
            no_state.false_block_count,
        ),
        additional_manual_interventions: signed_delta(
            stateful.manual_intervention_count,
            no_state.manual_intervention_count,
        ),
        additional_coordination_friction_events: signed_delta(
            coordination_friction_events(stateful),
            coordination_friction_events(no_state),
        ),
        additional_wall_time_ms: signed_delta(stateful.wall_time_ms, no_state.wall_time_ms),
    }
}

fn coordination_friction_events(summary: &ModeComparisonSummary) -> u64 {
    summary
        .coordinated_blocks
        .saturating_add(summary.denied_writes)
        .saturating_add(summary.scope_mismatches)
        .saturating_add(summary.stale_intents)
        .saturating_add(summary.timeouts)
        .saturating_add(summary.long_idle_periods)
        .saturating_add(summary.false_block_count)
        .saturating_add(summary.manual_intervention_count)
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn signed_delta(left: u64, right: u64) -> i64 {
    let delta = i128::from(left) - i128::from(right);
    delta.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn comparison_evidence_kind(
    kinds: impl IntoIterator<Item = BenchmarkEvidenceKind>,
) -> BenchmarkEvidenceKind {
    let mut iter = kinds.into_iter();
    let Some(first) = iter.next() else {
        return BenchmarkEvidenceKind::PairedAgentRun;
    };
    if iter.all(|kind| kind == first) {
        first
    } else {
        BenchmarkEvidenceKind::Mixed
    }
}

fn empirical_claim_allowed(kind: BenchmarkEvidenceKind) -> bool {
    matches!(kind, BenchmarkEvidenceKind::PairedAgentRun)
}

fn evidence_notes(kind: BenchmarkEvidenceKind) -> Vec<String> {
    match kind {
        BenchmarkEvidenceKind::PairedAgentRun => vec![
            "paired_agent_run evidence comes from executed agent pairs; effect-size claims still require an overlap-focused manifest, enough paired-valid samples, and overhead reporting."
                .to_string(),
        ],
        BenchmarkEvidenceKind::SyntheticFixture => vec![
            "synthetic_fixture evidence is a scripted fixture for validating report plumbing; do not cite synthetic deltas as empirical product efficacy or performance evidence."
                .to_string(),
            "Run paired_agent_run comparisons on exact_file_overlap or same_directory manifests before claiming prevented conflicts."
                .to_string(),
        ],
        BenchmarkEvidenceKind::Mixed => vec![
            "mixed evidence combines scripted fixtures with executed paired-agent runs; do not aggregate it into empirical efficacy claims."
                .to_string(),
            "Report synthetic_fixture and paired_agent_run results separately, with wall-time and token overhead."
                .to_string(),
        ],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticOptions {
    pub output_dir: PathBuf,
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyntheticBenchmarkArtifacts {
    pub manifest_path: PathBuf,
    pub stateful_run_dir: PathBuf,
    pub no_state_run_dir: PathBuf,
    pub comparison: ComparisonReport,
}

pub fn run_synthetic_benchmark(options: SyntheticOptions) -> Result<SyntheticBenchmarkArtifacts> {
    fs::create_dir_all(&options.output_dir)?;
    let scenarios = synthetic_scenarios();
    let manifest = scenarios
        .iter()
        .map(|scenario| scenario.pair.clone())
        .collect::<Vec<_>>();
    let run_id = sanitize_id(&options.run_id);
    let manifest_path = options.output_dir.join(format!("{run_id}-manifest.jsonl"));
    let stateful_run_id = format!("{run_id}-stateful");
    let no_state_run_id = format!("{run_id}-no-state");
    let stateful_run_dir = options.output_dir.join(&stateful_run_id);
    let no_state_run_dir = options.output_dir.join(&no_state_run_id);

    write_jsonl(&manifest_path, &manifest)?;
    write_synthetic_run(
        &stateful_run_dir,
        &stateful_run_id,
        RunMode::Stateful,
        &scenarios,
    )?;
    write_synthetic_run(
        &no_state_run_dir,
        &no_state_run_id,
        RunMode::NoState,
        &scenarios,
    )?;

    let comparison = compare_runs(CompareOptions {
        stateful_run_dir: vec![stateful_run_dir.clone()],
        no_state_run_dir: vec![no_state_run_dir.clone()],
        awareness_run_dir: Vec::new(),
        manifest: manifest_path.clone(),
        max_pairs: None,
    })?;

    Ok(SyntheticBenchmarkArtifacts {
        manifest_path,
        stateful_run_dir,
        no_state_run_dir,
        comparison,
    })
}

#[derive(Debug, Clone)]
struct SyntheticScenario {
    pair: PairManifestEntry,
    stateful: SyntheticModeOutcome,
    no_state: SyntheticModeOutcome,
}

#[derive(Debug, Clone)]
struct SyntheticModeOutcome {
    task_a: HarnessTaskOutcome,
    task_b: HarnessTaskOutcome,
    wall_time_ms: u64,
    expected_document: &'static str,
    live_document: &'static str,
    reload_document: &'static str,
    metrics: Value,
    events: Vec<Value>,
}

fn synthetic_scenarios() -> Vec<SyntheticScenario> {
    vec![
        synthetic_scenario(
            "same-position-insert",
            "Both users insert at offset 0 from the same base document.",
            "doc.txt",
            "AB\n",
            no_state_regression(
                "AB\n",
                HarnessTaskOutcome::Failed,
                HarnessTaskOutcome::Passed,
                "B\n",
                "B\n",
                serde_json::json!({
                    "local_latency_p95_ms": 42,
                    "canonical_match": false,
                    "reordered_delivery_converged": false
                }),
                true,
            ),
            stateful_success(
                "AB\n",
                serde_json::json!({
                    "local_latency_p95_ms": 78,
                    "canonical_match": true,
                    "reordered_delivery_converged": true
                }),
            ),
        ),
        synthetic_scenario(
            "delete-while-insert",
            "Agent A deletes a line while agent B inserts relative to that line.",
            "doc.txt",
            "title\ninserted\n",
            no_state_regression(
                "title\ninserted\n",
                HarnessTaskOutcome::Passed,
                HarnessTaskOutcome::Failed,
                "title\n",
                "title\n",
                serde_json::json!({
                    "local_latency_p95_ms": 47,
                    "canonical_match": false,
                    "intent_preserved": false
                }),
                true,
            ),
            stateful_success(
                "title\ninserted\n",
                serde_json::json!({
                    "local_latency_p95_ms": 86,
                    "canonical_match": true,
                    "intent_preserved": true
                }),
            ),
        ),
        synthetic_scenario(
            "offline-reconnect-sync",
            "One client edits offline and reconnects after remote edits are already visible.",
            "doc.txt",
            "base\nremote\noffline\n",
            no_state_regression(
                "base\nremote\noffline\n",
                HarnessTaskOutcome::Failed,
                HarnessTaskOutcome::Passed,
                "base\nremote\n",
                "base\nremote\n",
                serde_json::json!({
                    "local_latency_p95_ms": 51,
                    "offline_reconnect_converged": false,
                    "missing_offline_ops": 1
                }),
                true,
            ),
            stateful_success(
                "base\nremote\noffline\n",
                serde_json::json!({
                    "local_latency_p95_ms": 93,
                    "offline_reconnect_converged": true,
                    "missing_offline_ops": 0
                }),
            ),
        ),
        synthetic_scenario(
            "duplicate-message-idempotency",
            "A delivered edit is replayed after reconnect and must be idempotent.",
            "doc.txt",
            "base\nedit\n",
            no_state_regression(
                "base\nedit\n",
                HarnessTaskOutcome::Failed,
                HarnessTaskOutcome::Passed,
                "base\nedit\nedit\n",
                "base\nedit\nedit\n",
                serde_json::json!({
                    "local_latency_p95_ms": 39,
                    "duplicate_messages_applied": 1,
                    "idempotent_replay": false
                }),
                true,
            ),
            stateful_success(
                "base\nedit\n",
                serde_json::json!({
                    "local_latency_p95_ms": 81,
                    "duplicate_messages_applied": 0,
                    "idempotent_replay": true
                }),
            ),
        ),
        synthetic_scenario(
            "save-reload-consistency",
            "Live state and persisted reload state must produce the same canonical document.",
            "doc.txt",
            "base\nA\nB\n",
            SyntheticModeOutcome {
                task_a: HarnessTaskOutcome::Passed,
                task_b: HarnessTaskOutcome::Failed,
                wall_time_ms: 64,
                expected_document: "base\nA\nB\n",
                live_document: "base\nA\nB\n",
                reload_document: "base\nA\n",
                metrics: serde_json::json!({
                    "local_latency_p95_ms": 44,
                    "save_reload_match": false,
                    "canonical_match": false
                }),
                events: vec![serde_json::json!({
                    "event_type": "save_reload_mismatch",
                    "path": "doc.txt"
                })],
            },
            stateful_success(
                "base\nA\nB\n",
                serde_json::json!({
                    "local_latency_p95_ms": 88,
                    "save_reload_match": true,
                    "canonical_match": true
                }),
            ),
        ),
    ]
}

fn synthetic_scenario(
    pair_id: &'static str,
    problem: &'static str,
    file: &'static str,
    expected_document: &'static str,
    no_state: SyntheticModeOutcome,
    stateful: SyntheticModeOutcome,
) -> SyntheticScenario {
    SyntheticScenario {
        pair: synthetic_pair(pair_id, problem, file, expected_document),
        stateful,
        no_state,
    }
}

fn no_state_regression(
    expected_document: &'static str,
    task_a: HarnessTaskOutcome,
    task_b: HarnessTaskOutcome,
    live_document: &'static str,
    reload_document: &'static str,
    metrics: Value,
    lost_edit: bool,
) -> SyntheticModeOutcome {
    let mut events = vec![serde_json::json!({
        "event_type": "uncoordinated_same_file_write_collision",
        "path": "doc.txt"
    })];
    if lost_edit {
        events.push(serde_json::json!({
            "event_type": "lost_edit_event",
            "path": "doc.txt"
        }));
    }

    SyntheticModeOutcome {
        task_a,
        task_b,
        wall_time_ms: 58,
        expected_document,
        live_document,
        reload_document,
        metrics,
        events,
    }
}

fn stateful_success(canonical_document: &'static str, metrics: Value) -> SyntheticModeOutcome {
    SyntheticModeOutcome {
        task_a: HarnessTaskOutcome::Passed,
        task_b: HarnessTaskOutcome::Passed,
        wall_time_ms: 96,
        expected_document: canonical_document,
        live_document: canonical_document,
        reload_document: canonical_document,
        metrics,
        events: vec![
            serde_json::json!({
                "event_type": "coordinated_block",
                "path": "doc.txt"
            }),
            serde_json::json!({
                "event_type": "denied_write",
                "path": "doc.txt",
                "reason": "retry_required"
            }),
        ],
    }
}

fn synthetic_pair(
    pair_id: &str,
    problem: &str,
    file: &str,
    expected_document: &str,
) -> PairManifestEntry {
    let task_a = synthetic_instance(
        &format!("{pair_id}-agent-a"),
        &format!("{problem}\nAgent A expected canonical document:\n{expected_document}"),
        file,
    );
    let task_b = synthetic_instance(
        &format!("{pair_id}-agent-b"),
        &format!("{problem}\nAgent B expected canonical document:\n{expected_document}"),
        file,
    );

    PairManifestEntry {
        pair_id: pair_id.to_string(),
        repo: "synthetic/concurrent-editor".to_string(),
        base_commit: Some("synthetic-base".to_string()),
        version: Some("synthetic-v1".to_string()),
        eligibility: PairEligibility::SameBaseCommit,
        class: PairClass::ExactFileOverlap,
        task_a_files: vec![file.to_string()],
        task_b_files: vec![file.to_string()],
        task_a,
        task_b,
    }
}

fn synthetic_instance(instance_id: &str, problem_statement: &str, file: &str) -> SweBenchInstance {
    SweBenchInstance {
        instance_id: instance_id.to_string(),
        repo: "synthetic/concurrent-editor".to_string(),
        base_commit: "synthetic-base".to_string(),
        problem_statement: problem_statement.to_string(),
        version: Some("synthetic-v1".to_string()),
        patch: format!("diff --git a/{file} b/{file}\n"),
        test_patch: String::new(),
        fail_to_pass: Vec::new(),
        pass_to_pass: Vec::new(),
        difficulty: Some("synthetic".to_string()),
    }
}

fn write_synthetic_run(
    run_dir: &Path,
    run_id: &str,
    mode: RunMode,
    scenarios: &[SyntheticScenario],
) -> Result<()> {
    fs::create_dir_all(run_dir)?;
    write_json_file(
        run_dir.join("run.json"),
        &RunMetadata {
            run_id: run_id.to_string(),
            mode,
            evidence_kind: BenchmarkEvidenceKind::SyntheticFixture,
        },
    )?;

    for scenario in scenarios {
        write_synthetic_pair_run(run_dir, mode, scenario)?;
    }

    Ok(())
}

fn write_synthetic_pair_run(
    run_dir: &Path,
    mode: RunMode,
    scenario: &SyntheticScenario,
) -> Result<()> {
    let outcome = match mode {
        RunMode::Stateful | RunMode::Awareness => &scenario.stateful,
        RunMode::NoState => &scenario.no_state,
    };
    let pair_dir = run_dir.join(sanitize_id(&scenario.pair.pair_id));
    fs::create_dir_all(&pair_dir)?;
    write_json_file(pair_dir.join("pair.json"), &scenario.pair)?;
    write_json_file(pair_dir.join("task-a.json"), &scenario.pair.task_a)?;
    write_json_file(pair_dir.join("task-b.json"), &scenario.pair.task_b)?;
    fs::write(
        pair_dir.join("combined.patch"),
        format!(
            "synthetic mode: {}\npair: {}\nexpected:\n{}live:\n{}reload:\n{}",
            mode.as_str(),
            scenario.pair.pair_id,
            outcome.expected_document,
            outcome.live_document,
            outcome.reload_document
        ),
    )?;

    write_json_file(
        pair_dir.join("harness-result.json"),
        &serde_json::json!({
            "task_results": [
                {
                    "instance_id": scenario.pair.task_a.instance_id,
                    "status": outcome_status(outcome.task_a),
                },
                {
                    "instance_id": scenario.pair.task_b.instance_id,
                    "status": outcome_status(outcome.task_b),
                }
            ],
            "metrics": outcome.metrics,
            "synthetic": {
                "expected_document": outcome.expected_document,
                "live_document": outcome.live_document,
                "reload_document": outcome.reload_document,
                "canonical_match": outcome.expected_document == outcome.live_document,
                "reload_matches_live": outcome.live_document == outcome.reload_document
            }
        }),
    )?;

    let mut events = vec![
        serde_json::json!({
            "event_type": "pair_started",
            "pair_id": scenario.pair.pair_id,
            "mode": mode
        }),
        serde_json::json!({"event_type":"agent_started","agent_id":"agent-a"}),
        serde_json::json!({"event_type":"agent_started","agent_id":"agent-b"}),
    ];
    events.extend(outcome.events.clone());
    events.push(serde_json::json!({
        "event_type": "pair_finished",
        "wall_time_ms": outcome.wall_time_ms
    }));
    write_jsonl(pair_dir.join("observer-events.jsonl"), &events)?;

    write_json_file(
        pair_dir.join("pair-run.json"),
        &PairRunRecord {
            pair_id: scenario.pair.pair_id.clone(),
            mode,
            agent_a: AgentRunRecord::finished("agent-a", 0, outcome.wall_time_ms / 2),
            agent_b: AgentRunRecord::finished("agent-b", 0, outcome.wall_time_ms / 2),
            agents: vec![
                AgentRunRecord::finished("agent-a", 0, outcome.wall_time_ms / 2),
                AgentRunRecord::finished("agent-b", 0, outcome.wall_time_ms / 2),
            ],
            wall_time_ms: outcome.wall_time_ms,
            combined_patch_path: "combined.patch".to_string(),
            harness_result_path: Some("harness-result.json".to_string()),
            error: None,
        },
    )?;

    Ok(())
}

fn outcome_status(outcome: HarnessTaskOutcome) -> &'static str {
    match outcome {
        HarnessTaskOutcome::Passed => "passed",
        HarnessTaskOutcome::Failed => "failed",
        HarnessTaskOutcome::SetupError => "setup_error",
        HarnessTaskOutcome::Unknown => "unknown",
    }
}

fn build_reports(run_dirs: &[PathBuf]) -> Result<Vec<RunReport>> {
    if run_dirs.is_empty() {
        anyhow::bail!("at least one run dir is required");
    }
    run_dirs.iter().map(build_report).collect()
}

fn joined_run_ids(reports: &[RunReport]) -> String {
    reports
        .iter()
        .map(|report| report.run_id.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn pair_report_map(reports: &[RunReport]) -> BTreeMap<String, PairReport> {
    let mut pairs = BTreeMap::new();
    for report in reports {
        for pair in &report.pairs {
            pairs.insert(pair.pair_id.clone(), pair.clone());
        }
    }
    pairs
}

fn summarize_mode_for_manifest(
    run_id: String,
    pair_ids: &[String],
    pairs: &BTreeMap<String, PairReport>,
) -> ModeComparisonSummary {
    let mut summary = ModeComparisonSummary {
        run_id,
        artifact_pairs: 0,
        missing_artifacts: 0,
        scored_pairs: 0,
        setup_error_pairs: 0,
        unknown_pairs: 0,
        available_valid_score: None,
        raw_manifest_score: 0.0,
        infra_loss_rate: 0.0,
        task_passed: 0,
        task_failed: 0,
        setup_errors: 0,
        unknown_task_results: 0,
        uncoordinated_same_file_collisions: 0,
        coordinated_blocks: 0,
        lost_edit_events: 0,
        denied_writes: 0,
        scope_mismatches: 0,
        stale_intents: 0,
        timeouts: 0,
        long_idle_periods: 0,
        authorization_warnings: 0,
        warned_writes_applied: 0,
        wait_events: 0,
        wall_time_ms: 0,
        token_count: None,
        tool_call_count: None,
        preserved_edit_count: 0,
        missing_expected_line_count: 0,
        false_block_count: 0,
        missed_conflict_count: 0,
        manual_intervention_count: 0,
        time_to_converge_ms: None,
    };
    let mut score_sum = 0.0;
    let mut token_count = 0u64;
    let mut tool_call_count = 0u64;
    let mut time_to_converge_ms = 0u64;
    let mut has_tokens = false;
    let mut has_tool_calls = false;
    let mut has_time_to_converge = false;

    for pair_id in pair_ids {
        let Some(pair) = pairs.get(pair_id) else {
            summary.missing_artifacts += 1;
            continue;
        };

        summary.artifact_pairs += 1;
        summary.wall_time_ms += pair.wall_time_ms;
        summary.uncoordinated_same_file_collisions +=
            pair.collisions.uncoordinated_same_file_collisions;
        summary.coordinated_blocks += pair.collisions.coordinated_blocks;
        summary.lost_edit_events += pair.collisions.lost_edit_events;
        summary.denied_writes += pair.collisions.denied_writes;
        summary.scope_mismatches += pair.collisions.scope_mismatches;
        summary.stale_intents += pair.collisions.stale_intents;
        summary.timeouts += pair.collisions.timeouts;
        summary.long_idle_periods += pair.collisions.long_idle_periods;
        summary.authorization_warnings += pair.collisions.authorization_warnings;
        summary.warned_writes_applied += pair.collisions.warned_writes_applied;
        summary.wait_events += pair.collisions.wait_events;
        summary.preserved_edit_count += pair.preserved_edit_count;
        summary.missing_expected_line_count += pair.missing_expected_line_count;
        summary.false_block_count += pair.false_block_count;
        summary.missed_conflict_count += pair.missed_conflict_count;
        summary.manual_intervention_count += pair.manual_intervention_count;
        for outcome in &pair.task_outcomes {
            count_comparison_task_outcome(*outcome, &mut summary);
        }

        if let Some(count) = pair.token_count {
            has_tokens = true;
            token_count += count;
        }
        if let Some(count) = pair.tool_call_count {
            has_tool_calls = true;
            tool_call_count += count;
        }
        if let Some(count) = pair.time_to_converge_ms {
            has_time_to_converge = true;
            time_to_converge_ms += count;
        }

        match comparison_status(Some(pair)) {
            PairComparisonStatus::Scored => {
                summary.scored_pairs += 1;
                score_sum += pair
                    .score
                    .as_ref()
                    .map(|score| score.functional_pair_score)
                    .unwrap_or(0.0);
            }
            PairComparisonStatus::SetupError => summary.setup_error_pairs += 1,
            PairComparisonStatus::Unknown => summary.unknown_pairs += 1,
            PairComparisonStatus::MissingArtifact => summary.missing_artifacts += 1,
        }
    }

    summary.available_valid_score = average_score(score_sum, summary.scored_pairs);
    if !pair_ids.is_empty() {
        summary.raw_manifest_score = round3(score_sum / pair_ids.len() as f64);
        summary.infra_loss_rate = round3(
            (summary.missing_artifacts + summary.setup_error_pairs + summary.unknown_pairs) as f64
                / pair_ids.len() as f64,
        );
    }
    summary.token_count = has_tokens.then_some(token_count);
    summary.tool_call_count = has_tool_calls.then_some(tool_call_count);
    summary.time_to_converge_ms = has_time_to_converge.then_some(time_to_converge_ms);

    summary
}

fn count_comparison_task_outcome(outcome: HarnessTaskOutcome, summary: &mut ModeComparisonSummary) {
    match outcome {
        HarnessTaskOutcome::Passed => summary.task_passed += 1,
        HarnessTaskOutcome::Failed => summary.task_failed += 1,
        HarnessTaskOutcome::SetupError => summary.setup_errors += 1,
        HarnessTaskOutcome::Unknown => summary.unknown_task_results += 1,
    }
}

fn comparison_status(pair: Option<&PairReport>) -> PairComparisonStatus {
    let Some(pair) = pair else {
        return PairComparisonStatus::MissingArtifact;
    };
    if pair
        .task_outcomes
        .iter()
        .any(|outcome| matches!(outcome, HarnessTaskOutcome::SetupError))
    {
        return PairComparisonStatus::SetupError;
    }
    if pair
        .task_outcomes
        .iter()
        .any(|outcome| matches!(outcome, HarnessTaskOutcome::Unknown))
    {
        return PairComparisonStatus::Unknown;
    }
    if pair.score.is_some() {
        PairComparisonStatus::Scored
    } else {
        PairComparisonStatus::Unknown
    }
}

fn functional_pair_score(pair: Option<&PairReport>) -> Option<f64> {
    pair.and_then(|pair| pair.score.as_ref().map(|score| score.functional_pair_score))
}

fn average_score(score_sum: f64, count: usize) -> Option<f64> {
    (count > 0).then_some(round3(score_sum / count as f64))
}

fn exclusion_reason(
    stateful_status: PairComparisonStatus,
    no_state_status: PairComparisonStatus,
) -> &'static str {
    match (stateful_status, no_state_status) {
        (PairComparisonStatus::MissingArtifact, PairComparisonStatus::MissingArtifact) => {
            "missing_both_artifacts"
        }
        (PairComparisonStatus::MissingArtifact, _) => "missing_stateful_artifact",
        (_, PairComparisonStatus::MissingArtifact) => "missing_no_state_artifact",
        (PairComparisonStatus::SetupError, PairComparisonStatus::SetupError) => "setup_error_both",
        (PairComparisonStatus::SetupError, _) => "invalid_stateful_setup_error",
        (_, PairComparisonStatus::SetupError) => "invalid_no_state_setup_error",
        (PairComparisonStatus::Unknown, PairComparisonStatus::Unknown) => "unknown_both",
        (PairComparisonStatus::Unknown, _) => "invalid_stateful_unknown",
        (_, PairComparisonStatus::Unknown) => "invalid_no_state_unknown",
        (PairComparisonStatus::Scored, PairComparisonStatus::Scored) => "paired_valid",
    }
}

fn render_comparison_markdown(report: &ComparisonReport) -> String {
    let mut output = String::new();
    output.push_str("# Stateful Bench Comparison\n\n");
    output.push_str(&format!("- Manifest pairs: {}\n", report.manifest_pairs));
    output.push_str(&format!("- Stateful run: {}\n", report.stateful_run_id));
    output.push_str(&format!("- No-state run: {}\n", report.no_state_run_id));
    output.push_str(&format!("- Evidence kind: {}\n", report.evidence_kind));
    output.push_str(&format!(
        "- Empirical claim allowed: {}\n",
        if report.empirical_claim_allowed {
            "yes"
        } else {
            "no"
        }
    ));
    for note in &report.evidence_notes {
        output.push_str(&format!("- Evidence note: {note}\n"));
    }
    output.push('\n');

    output.push_str("## Paired Valid\n\n");
    output.push_str("| Metric | Value |\n");
    output.push_str("| --- | ---: |\n");
    output.push_str(&format!(
        "| Paired valid pairs | {} |\n",
        report.paired.paired_valid_pairs
    ));
    output.push_str(&format!(
        "| Stateful functional score | {} |\n",
        format_optional_score(report.paired.stateful_functional_score)
    ));
    output.push_str(&format!(
        "| No-state functional score | {} |\n",
        format_optional_score(report.paired.no_state_functional_score)
    ));
    output.push_str(&format!(
        "| Paired valid functional delta | {} |\n",
        format_optional_score(report.paired.paired_valid_functional_delta)
    ));
    output.push_str(&format!(
        "| Raw manifest functional delta | {} |\n\n",
        format_optional_score(report.paired.raw_manifest_functional_delta)
    ));

    output.push_str("## Coordination Effects\n\n");
    output.push_str("| Metric | Delta |\n");
    output.push_str("| --- | ---: |\n");
    output.push_str(&format!(
        "| Prevented uncoordinated same-file collisions | {} |\n",
        report
            .coordination_effects
            .prevented_uncoordinated_same_file_collisions
    ));
    output.push_str(&format!(
        "| Prevented lost edit events | {} |\n",
        report.coordination_effects.prevented_lost_edit_events
    ));
    output.push_str(&format!(
        "| Additional coordinated blocks | {} |\n",
        report.coordination_effects.additional_coordinated_blocks
    ));
    output.push_str(&format!(
        "| Additional denied writes | {} |\n",
        report.coordination_effects.additional_denied_writes
    ));
    output.push_str(&format!(
        "| Additional scope mismatches | {} |\n",
        report.coordination_effects.additional_scope_mismatches
    ));
    output.push_str(&format!(
        "| Additional stale intents | {} |\n",
        report.coordination_effects.additional_stale_intents
    ));
    output.push_str(&format!(
        "| Additional timeouts | {} |\n",
        report.coordination_effects.additional_timeouts
    ));
    output.push_str(&format!(
        "| Additional long idle periods | {} |\n",
        report.coordination_effects.additional_long_idle_periods
    ));
    output.push_str(&format!(
        "| Additional false blocks | {} |\n",
        report.coordination_effects.additional_false_blocks
    ));
    output.push_str(&format!(
        "| Additional manual interventions | {} |\n",
        report.coordination_effects.additional_manual_interventions
    ));
    output.push_str(&format!(
        "| Additional coordination friction events | {} |\n",
        report
            .coordination_effects
            .additional_coordination_friction_events
    ));
    output.push_str(&format!(
        "| Additional wall time ms | {} |\n\n",
        report.coordination_effects.additional_wall_time_ms
    ));

    output.push_str("## Mode Metrics\n\n");
    if let Some(awareness) = &report.awareness {
        output.push_str("| Metric | Stateful | Awareness | No-state |\n");
        output.push_str("| --- | ---: | ---: | ---: |\n");
        output.push_str(&format!(
            "| Artifact pairs | {} | {} | {} |\n",
            report.stateful.artifact_pairs,
            awareness.artifact_pairs,
            report.no_state.artifact_pairs
        ));
        output.push_str(&format!(
            "| Scored pairs | {} | {} | {} |\n",
            report.stateful.scored_pairs, awareness.scored_pairs, report.no_state.scored_pairs
        ));
        output.push_str(&format!(
            "| Available valid functional score | {} | {} | {} |\n",
            format_optional_score(report.stateful.available_valid_score),
            format_optional_score(awareness.available_valid_score),
            format_optional_score(report.no_state.available_valid_score)
        ));
        output.push_str(&format!(
            "| Raw manifest functional score | {:.3} | {:.3} | {:.3} |\n",
            report.stateful.raw_manifest_score,
            awareness.raw_manifest_score,
            report.no_state.raw_manifest_score
        ));
        output.push_str(&format!(
            "| Missing artifacts | {} | {} | {} |\n",
            report.stateful.missing_artifacts,
            awareness.missing_artifacts,
            report.no_state.missing_artifacts
        ));
        output.push_str(&format!(
            "| Setup error pairs | {} | {} | {} |\n",
            report.stateful.setup_error_pairs,
            awareness.setup_error_pairs,
            report.no_state.setup_error_pairs
        ));
        output.push_str(&format!(
            "| Unknown pairs | {} | {} | {} |\n",
            report.stateful.unknown_pairs, awareness.unknown_pairs, report.no_state.unknown_pairs
        ));
        output.push_str(&format!(
            "| Infra loss rate | {:.3} | {:.3} | {:.3} |\n",
            report.stateful.infra_loss_rate,
            awareness.infra_loss_rate,
            report.no_state.infra_loss_rate
        ));
        output.push_str(&format!(
            "| Uncoordinated same-file collisions | {} | {} | {} |\n",
            report.stateful.uncoordinated_same_file_collisions,
            awareness.uncoordinated_same_file_collisions,
            report.no_state.uncoordinated_same_file_collisions
        ));
        output.push_str(&format!(
            "| Lost edit events | {} | {} | {} |\n",
            report.stateful.lost_edit_events,
            awareness.lost_edit_events,
            report.no_state.lost_edit_events
        ));
        output.push_str(&format!(
            "| Coordinated blocks | {} | {} | {} |\n",
            report.stateful.coordinated_blocks,
            awareness.coordinated_blocks,
            report.no_state.coordinated_blocks
        ));
        output.push_str(&format!(
            "| Denied writes | {} | {} | {} |\n",
            report.stateful.denied_writes, awareness.denied_writes, report.no_state.denied_writes
        ));
        output.push_str(&format!(
            "| Authorization warnings | {} | {} | {} |\n",
            report.stateful.authorization_warnings,
            awareness.authorization_warnings,
            report.no_state.authorization_warnings
        ));
        output.push_str(&format!(
            "| Warned writes applied | {} | {} | {} |\n",
            report.stateful.warned_writes_applied,
            awareness.warned_writes_applied,
            report.no_state.warned_writes_applied
        ));
        output.push_str(&format!(
            "| Wait events | {} | {} | {} |\n",
            report.stateful.wait_events, awareness.wait_events, report.no_state.wait_events
        ));
        output.push_str(&format!(
            "| Preserved edit count | {} | {} | {} |\n",
            report.stateful.preserved_edit_count,
            awareness.preserved_edit_count,
            report.no_state.preserved_edit_count
        ));
        output.push_str(&format!(
            "| Missing expected line count | {} | {} | {} |\n",
            report.stateful.missing_expected_line_count,
            awareness.missing_expected_line_count,
            report.no_state.missing_expected_line_count
        ));
        output.push_str(&format!(
            "| False block count | {} | {} | {} |\n",
            report.stateful.false_block_count,
            awareness.false_block_count,
            report.no_state.false_block_count
        ));
        output.push_str(&format!(
            "| Missed conflict count | {} | {} | {} |\n",
            report.stateful.missed_conflict_count,
            awareness.missed_conflict_count,
            report.no_state.missed_conflict_count
        ));
        output.push_str(&format!(
            "| Manual intervention count | {} | {} | {} |\n",
            report.stateful.manual_intervention_count,
            awareness.manual_intervention_count,
            report.no_state.manual_intervention_count
        ));
        output.push_str(&format!(
            "| Time to converge ms | {} | {} | {} |\n\n",
            format_optional_count(report.stateful.time_to_converge_ms),
            format_optional_count(awareness.time_to_converge_ms),
            format_optional_count(report.no_state.time_to_converge_ms)
        ));
    } else {
        output.push_str("| Metric | Stateful | No-state |\n");
        output.push_str("| --- | ---: | ---: |\n");
        output.push_str(&format!(
            "| Artifact pairs | {} | {} |\n",
            report.stateful.artifact_pairs, report.no_state.artifact_pairs
        ));
        output.push_str(&format!(
            "| Scored pairs | {} | {} |\n",
            report.stateful.scored_pairs, report.no_state.scored_pairs
        ));
        output.push_str(&format!(
            "| Available valid functional score | {} | {} |\n",
            format_optional_score(report.stateful.available_valid_score),
            format_optional_score(report.no_state.available_valid_score)
        ));
        output.push_str(&format!(
            "| Raw manifest functional score | {:.3} | {:.3} |\n",
            report.stateful.raw_manifest_score, report.no_state.raw_manifest_score
        ));
        output.push_str(&format!(
            "| Missing artifacts | {} | {} |\n",
            report.stateful.missing_artifacts, report.no_state.missing_artifacts
        ));
        output.push_str(&format!(
            "| Setup error pairs | {} | {} |\n",
            report.stateful.setup_error_pairs, report.no_state.setup_error_pairs
        ));
        output.push_str(&format!(
            "| Unknown pairs | {} | {} |\n",
            report.stateful.unknown_pairs, report.no_state.unknown_pairs
        ));
        output.push_str(&format!(
            "| Infra loss rate | {:.3} | {:.3} |\n",
            report.stateful.infra_loss_rate, report.no_state.infra_loss_rate
        ));
        output.push_str(&format!(
            "| Uncoordinated same-file collisions | {} | {} |\n",
            report.stateful.uncoordinated_same_file_collisions,
            report.no_state.uncoordinated_same_file_collisions
        ));
        output.push_str(&format!(
            "| Lost edit events | {} | {} |\n",
            report.stateful.lost_edit_events, report.no_state.lost_edit_events
        ));
        output.push_str(&format!(
            "| Coordinated blocks | {} | {} |\n",
            report.stateful.coordinated_blocks, report.no_state.coordinated_blocks
        ));
        output.push_str(&format!(
            "| Denied writes | {} | {} |\n",
            report.stateful.denied_writes, report.no_state.denied_writes
        ));
        output.push_str(&format!(
            "| Preserved edit count | {} | {} |\n",
            report.stateful.preserved_edit_count, report.no_state.preserved_edit_count
        ));
        output.push_str(&format!(
            "| Missing expected line count | {} | {} |\n",
            report.stateful.missing_expected_line_count,
            report.no_state.missing_expected_line_count
        ));
        output.push_str(&format!(
            "| False block count | {} | {} |\n",
            report.stateful.false_block_count, report.no_state.false_block_count
        ));
        output.push_str(&format!(
            "| Missed conflict count | {} | {} |\n",
            report.stateful.missed_conflict_count, report.no_state.missed_conflict_count
        ));
        output.push_str(&format!(
            "| Manual intervention count | {} | {} |\n",
            report.stateful.manual_intervention_count, report.no_state.manual_intervention_count
        ));
        output.push_str(&format!(
            "| Time to converge ms | {} | {} |\n\n",
            format_optional_count(report.stateful.time_to_converge_ms),
            format_optional_count(report.no_state.time_to_converge_ms)
        ));
    }

    if !report.excluded_pairs.is_empty() {
        output.push_str("## Excluded Pairs\n\n");
        output.push_str("| Pair | Stateful | No-state | Reason |\n");
        output.push_str("| --- | --- | --- | --- |\n");
        for pair in &report.excluded_pairs {
            output.push_str(&format!(
                "| {} | {:?} | {:?} | {} |\n",
                pair.pair_id, pair.stateful_status, pair.no_state_status, pair.reason
            ));
        }
    }

    output
}

fn format_optional_score(score: Option<f64>) -> String {
    score
        .map(|score| format!("{score:.3}"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_optional_count(count: Option<u64>) -> String {
    count
        .map(|count| count.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReportSummary {
    pub pairs_total: usize,
    pub pairs_scored: usize,
    pub composite_coordination_score: f64,
    pub functional_pair_score: f64,
    pub collision_safety_score: f64,
    pub coordination_cost_score: f64,
    pub task_passed: usize,
    pub task_failed: usize,
    pub setup_errors: usize,
    pub unknown_task_results: usize,
    pub uncoordinated_same_file_collisions: u64,
    pub coordinated_blocks: u64,
    pub lost_edit_events: u64,
    pub denied_writes: u64,
    pub scope_mismatches: u64,
    pub stale_intents: u64,
    pub timeouts: u64,
    pub long_idle_periods: u64,
    pub authorization_warnings: u64,
    pub warned_writes_applied: u64,
    pub wait_events: u64,
    pub wall_time_ms: u64,
    pub token_count: Option<u64>,
    pub tool_call_count: Option<u64>,
    pub preserved_edit_count: u64,
    pub missing_expected_line_count: u64,
    pub false_block_count: u64,
    pub missed_conflict_count: u64,
    pub manual_intervention_count: u64,
    pub time_to_converge_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairReport {
    pub pair_id: String,
    pub functional: FunctionalOutcome,
    #[serde(default)]
    pub task_outcomes: Vec<HarnessTaskOutcome>,
    pub collisions: CollisionMetrics,
    pub score: Option<CompositeScore>,
    pub wall_time_ms: u64,
    pub token_count: Option<u64>,
    pub tool_call_count: Option<u64>,
    pub preserved_edit_count: u64,
    pub missing_expected_line_count: u64,
    pub false_block_count: u64,
    pub missed_conflict_count: u64,
    pub manual_intervention_count: u64,
    pub time_to_converge_ms: Option<u64>,
}

pub fn build_report(run_dir: impl AsRef<Path>) -> Result<RunReport> {
    let run_dir = run_dir.as_ref();
    let metadata: RunMetadata = read_json_file(run_dir.join("run.json"))?;
    let mut pair_dirs = fs::read_dir(run_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| path.join("pair-run.json").is_file())
        .collect::<Vec<_>>();
    pair_dirs.sort();

    let mut pairs = Vec::new();
    let mut summary = ReportSummary::default();
    let mut score_sums = ScoreSums::default();
    let mut token_count = 0u64;
    let mut tool_call_count = 0u64;
    let mut time_to_converge_ms = 0u64;
    let mut has_tokens = false;
    let mut has_tool_calls = false;
    let mut has_time_to_converge = false;

    for pair_dir in pair_dirs {
        let record: PairRunRecord = read_json_file(pair_dir.join("pair-run.json"))?;
        let harness = record
            .harness_result_path
            .as_ref()
            .and_then(|path| read_harness_result(pair_dir.join(path)).ok())
            .unwrap_or_default();
        let mut collisions = read_collision_metrics(pair_dir.join("observer-events.jsonl"))?;
        if recorded_agents(&record)
            .iter()
            .any(|agent| agent.outcome == AgentOutcome::TimedOut)
        {
            collisions.timeouts += 1;
        }
        let task_outcomes = harness.task_outcomes(expected_task_count(&record));
        let functional = functional_outcome_from_task_outcomes(&task_outcomes);
        let score = composite_score_for_task_outcomes(&task_outcomes, &collisions);
        let agent_usage = aggregate_agent_usage_metrics(&record);
        let pair_token_count = harness.metrics.token_count.or(agent_usage.token_count);

        summary.wall_time_ms += record.wall_time_ms;
        summary.uncoordinated_same_file_collisions += collisions.uncoordinated_same_file_collisions;
        summary.coordinated_blocks += collisions.coordinated_blocks;
        summary.lost_edit_events += collisions.lost_edit_events;
        summary.denied_writes += collisions.denied_writes;
        summary.scope_mismatches += collisions.scope_mismatches;
        summary.stale_intents += collisions.stale_intents;
        summary.timeouts += collisions.timeouts;
        summary.long_idle_periods += collisions.long_idle_periods;
        summary.authorization_warnings += collisions.authorization_warnings;
        summary.warned_writes_applied += collisions.warned_writes_applied;
        summary.wait_events += collisions.wait_events;
        summary.preserved_edit_count += harness.metrics.preserved_edit_count;
        summary.missing_expected_line_count += harness.metrics.missing_expected_line_count;
        summary.false_block_count += harness.metrics.false_block_count;
        summary.missed_conflict_count += harness.metrics.missed_conflict_count;
        summary.manual_intervention_count += harness.metrics.manual_intervention_count;
        for outcome in &task_outcomes {
            count_task_outcome(*outcome, &mut summary);
        }

        if let Some(score) = &score {
            score_sums.add(score);
        }
        if let Some(count) = pair_token_count {
            has_tokens = true;
            token_count += count;
        }
        if let Some(count) = harness.metrics.tool_call_count {
            has_tool_calls = true;
            tool_call_count += count;
        }
        if let Some(count) = harness.metrics.time_to_converge_ms {
            has_time_to_converge = true;
            time_to_converge_ms += count;
        }

        pairs.push(PairReport {
            pair_id: record.pair_id,
            functional,
            task_outcomes,
            collisions,
            score,
            wall_time_ms: record.wall_time_ms,
            token_count: pair_token_count,
            tool_call_count: harness.metrics.tool_call_count,
            preserved_edit_count: harness.metrics.preserved_edit_count,
            missing_expected_line_count: harness.metrics.missing_expected_line_count,
            false_block_count: harness.metrics.false_block_count,
            missed_conflict_count: harness.metrics.missed_conflict_count,
            manual_intervention_count: harness.metrics.manual_intervention_count,
            time_to_converge_ms: harness.metrics.time_to_converge_ms,
        });
    }

    summary.pairs_total = pairs.len();
    summary.pairs_scored = score_sums.count;
    if score_sums.count > 0 {
        summary.composite_coordination_score =
            round3(score_sums.composite / score_sums.count as f64);
        summary.functional_pair_score = round3(score_sums.functional / score_sums.count as f64);
        summary.collision_safety_score = round3(score_sums.collision / score_sums.count as f64);
        summary.coordination_cost_score = round3(score_sums.cost / score_sums.count as f64);
    }
    summary.token_count = has_tokens.then_some(token_count);
    summary.tool_call_count = has_tool_calls.then_some(tool_call_count);
    summary.time_to_converge_ms = has_time_to_converge.then_some(time_to_converge_ms);

    Ok(RunReport {
        run_id: metadata.run_id,
        mode: metadata.mode,
        evidence_kind: metadata.evidence_kind,
        summary,
        pairs,
    })
}

fn ensure_run_has_scored_pairs(report: &RunReport) -> Result<()> {
    if report.summary.pairs_total > 0 && report.summary.pairs_scored == 0 {
        anyhow::bail!(
            "run completed with no scored pairs; setup_errors={}, unknown_task_results={}",
            report.summary.setup_errors,
            report.summary.unknown_task_results
        );
    }
    Ok(())
}

#[derive(Default)]
struct ScoreSums {
    count: usize,
    composite: f64,
    functional: f64,
    collision: f64,
    cost: f64,
}

impl ScoreSums {
    fn add(&mut self, score: &CompositeScore) {
        self.count += 1;
        self.composite += score.composite_coordination_score;
        self.functional += score.functional_pair_score;
        self.collision += score.collision_safety_score;
        self.cost += score.coordination_cost_score;
    }
}

fn count_task_outcome(outcome: HarnessTaskOutcome, summary: &mut ReportSummary) {
    match outcome {
        HarnessTaskOutcome::Passed => summary.task_passed += 1,
        HarnessTaskOutcome::Failed => summary.task_failed += 1,
        HarnessTaskOutcome::SetupError => summary.setup_errors += 1,
        HarnessTaskOutcome::Unknown => summary.unknown_task_results += 1,
    }
}

#[derive(Default)]
struct HarnessResult {
    task_results: Vec<HarnessTaskResult>,
    metrics: HarnessMetrics,
}

impl HarnessResult {
    fn task_outcomes(&self, expected_task_count: usize) -> Vec<HarnessTaskOutcome> {
        let mut outcomes = self
            .task_results
            .iter()
            .map(HarnessTaskResult::outcome)
            .collect::<Vec<_>>();
        outcomes.resize(
            outcomes.len().max(expected_task_count.max(2)),
            HarnessTaskOutcome::Unknown,
        );
        outcomes
    }
}

fn expected_task_count(record: &PairRunRecord) -> usize {
    recorded_agents(record).len().max(2)
}

fn recorded_agents(record: &PairRunRecord) -> Vec<&AgentRunRecord> {
    if record.agents.is_empty() {
        vec![&record.agent_a, &record.agent_b]
    } else {
        record.agents.iter().collect()
    }
}

fn aggregate_agent_usage_metrics(record: &PairRunRecord) -> AgentUsageMetrics {
    let mut metrics = AgentUsageMetrics::default();
    for agent in recorded_agents(record) {
        metrics.add_agent(agent);
    }
    metrics
}

fn functional_outcome_from_task_outcomes(outcomes: &[HarnessTaskOutcome]) -> FunctionalOutcome {
    FunctionalOutcome {
        task_a: outcomes
            .first()
            .copied()
            .unwrap_or(HarnessTaskOutcome::Unknown),
        task_b: outcomes
            .get(1)
            .copied()
            .unwrap_or(HarnessTaskOutcome::Unknown),
    }
}

#[derive(Default)]
struct HarnessTaskResult {
    status: Option<String>,
    setup_error: bool,
    fail_to_pass_passed: Option<bool>,
    pass_to_pass_passed: Option<bool>,
}

impl HarnessTaskResult {
    fn outcome(&self) -> HarnessTaskOutcome {
        if self.setup_error {
            return HarnessTaskOutcome::SetupError;
        }
        if let Some(status) = &self.status {
            return match status.as_str() {
                "passed" | "pass" | "resolved" => HarnessTaskOutcome::Passed,
                "failed" | "fail" | "unresolved" => HarnessTaskOutcome::Failed,
                "setup_error" | "setup-error" => HarnessTaskOutcome::SetupError,
                _ => HarnessTaskOutcome::Unknown,
            };
        }
        match (self.fail_to_pass_passed, self.pass_to_pass_passed) {
            (Some(true), Some(true)) => HarnessTaskOutcome::Passed,
            (Some(_), Some(_)) => HarnessTaskOutcome::Failed,
            _ => HarnessTaskOutcome::Unknown,
        }
    }
}

#[derive(Default)]
struct HarnessMetrics {
    token_count: Option<u64>,
    tool_call_count: Option<u64>,
    preserved_edit_count: u64,
    missing_expected_line_count: u64,
    false_block_count: u64,
    missed_conflict_count: u64,
    manual_intervention_count: u64,
    time_to_converge_ms: Option<u64>,
}

fn read_harness_result(path: impl AsRef<Path>) -> Result<HarnessResult> {
    let value: Value = read_json_file(path)?;
    let task_results = value
        .get("task_results")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| HarnessTaskResult {
                    status: item
                        .get("status")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    setup_error: item
                        .get("setup_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    fail_to_pass_passed: item.get("fail_to_pass_passed").and_then(Value::as_bool),
                    pass_to_pass_passed: item.get("pass_to_pass_passed").and_then(Value::as_bool),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let metrics = value.get("metrics").unwrap_or(&Value::Null);

    Ok(HarnessResult {
        task_results,
        metrics: HarnessMetrics {
            token_count: metrics.get("token_count").and_then(Value::as_u64),
            tool_call_count: metrics.get("tool_call_count").and_then(Value::as_u64),
            preserved_edit_count: metrics
                .get("preserved_edit_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            missing_expected_line_count: metrics
                .get("missing_expected_line_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            false_block_count: metrics
                .get("false_block_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            missed_conflict_count: metrics
                .get("missed_conflict_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            manual_intervention_count: metrics
                .get("manual_intervention_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            time_to_converge_ms: metrics.get("time_to_converge_ms").and_then(Value::as_u64),
        },
    })
}

fn read_collision_metrics(path: impl AsRef<Path>) -> Result<CollisionMetrics> {
    let path = path.as_ref();
    if !path.is_file() {
        return Ok(CollisionMetrics::default());
    }
    let events = read_jsonl::<Value>(path)?;
    let mut metrics = CollisionMetrics::default();
    for event in events {
        let event_type = event
            .get("event_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "uncoordinated_same_file_write_collision"
            | "same_file_overwrite_without_coordination" => {
                metrics.uncoordinated_same_file_collisions += 1;
            }
            "stateful_blocked_risky_write" | "coordinated_block" => {
                metrics.coordinated_blocks += 1;
            }
            "lost_edit" | "lost_edit_event" => metrics.lost_edit_events += 1,
            "denied_write" => {
                metrics.denied_writes += 1;
                if event
                    .get("reason")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| reason == "scope_mismatch")
                {
                    metrics.scope_mismatches += 1;
                }
            }
            "scope_mismatch" => metrics.scope_mismatches += 1,
            "stale_intent" | "intent_expired" => metrics.stale_intents += 1,
            "timeout" => metrics.timeouts += 1,
            "long_idle" | "long_idle_period" => metrics.long_idle_periods += 1,
            "authorization_warning" => metrics.authorization_warnings += 1,
            "warning_ignored_write" => metrics.warned_writes_applied += 1,
            "wait_event" => metrics.wait_events += 1,
            _ => {}
        }
    }
    Ok(metrics)
}

pub fn render_report_markdown(report: &RunReport) -> String {
    let mut output = String::new();
    output.push_str(&format!("# Stateful Bench Report: {}\n\n", report.run_id));
    output.push_str("| Metric | Value |\n");
    output.push_str("| --- | ---: |\n");
    output.push_str(&format!(
        "| Composite coordination score | {:.3} |\n",
        report.summary.composite_coordination_score
    ));
    output.push_str(&format!(
        "| Functional pair score | {:.3} |\n",
        report.summary.functional_pair_score
    ));
    output.push_str(&format!(
        "| Collision safety score | {:.3} |\n",
        report.summary.collision_safety_score
    ));
    output.push_str(&format!(
        "| Coordination cost score | {:.3} |\n",
        report.summary.coordination_cost_score
    ));
    output.push_str(&format!(
        "| Pairs scored | {} |\n",
        report.summary.pairs_scored
    ));
    output.push_str(&format!(
        "| Task passed | {} |\n",
        report.summary.task_passed
    ));
    output.push_str(&format!(
        "| Task failed | {} |\n",
        report.summary.task_failed
    ));
    output.push_str(&format!(
        "| Uncoordinated same-file collisions | {} |\n",
        report.summary.uncoordinated_same_file_collisions
    ));
    output.push_str(&format!(
        "| Lost edit events | {} |\n",
        report.summary.lost_edit_events
    ));
    output.push_str(&format!(
        "| Coordinated blocks | {} |\n",
        report.summary.coordinated_blocks
    ));
    output
}

fn read_json_file<T>(path: impl AsRef<Path>) -> Result<T>
where
    T: DeserializeOwned,
{
    let path = path.as_ref();
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read JSON file {}", path.display()))?;
    serde_json::from_str(&input)
        .with_context(|| format!("failed to parse JSON file {}", path.display()))
}

fn write_json_file<T>(path: impl AsRef<Path>, value: &T) -> Result<()>
where
    T: Serialize,
{
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn write_text_file(path: impl AsRef<Path>, text: &str) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn awareness_mode_uses_awareness_server_flag() {
        assert_eq!(RunMode::Awareness.as_str(), "awareness");
        assert_eq!(
            stateful_server_args(RunMode::Awareness, 3456, "workspace-1"),
            vec![
                "server".to_string(),
                "--host".to_string(),
                "127.0.0.1".to_string(),
                "--port".to_string(),
                "3456".to_string(),
                "--workspace-id".to_string(),
                "workspace-1".to_string(),
                "--coordination-mode".to_string(),
                "awareness".to_string(),
            ]
        );
        assert_eq!(
            stateful_server_args(RunMode::Stateful, 3456, "workspace-1"),
            vec![
                "server".to_string(),
                "--host".to_string(),
                "127.0.0.1".to_string(),
                "--port".to_string(),
                "3456".to_string(),
                "--workspace-id".to_string(),
                "workspace-1".to_string(),
            ]
        );
    }
}
