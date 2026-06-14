use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
            "stateful" => stateful = Some(parse_axis(value).context("invalid stateful axis")?),
            "subagent" => subagent = Some(parse_axis(value).context("invalid subagent axis")?),
            "config" => config_path = Some(PathBuf::from(value)),
            "env" => {
                let Some((env_key, env_value)) = value.split_once('=') else {
                    bail!("invalid DeNovoSWE env override `{value}`; expected KEY=VALUE");
                };
                env.insert(env_key.to_string(), env_value.to_string());
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
    pub stateful: bool,
    pub subagent: bool,
    pub total_instances: usize,
    pub completed_instances: usize,
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
    let correct_count = scores.iter().filter(|score| **score >= 1.0).count();
    let almost_correct_count = scores
        .iter()
        .filter(|score| **score >= 0.75 && **score < 1.0)
        .count();

    let condition_id = condition.id();
    let stateful = condition.stateful;
    let subagent = condition.subagent;

    DeNovoConditionReport {
        run_id: run_id.into(),
        condition_id,
        condition,
        stateful,
        subagent,
        total_instances,
        completed_instances,
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
    let by_axes = reports
        .iter()
        .map(|report| ((report.stateful, report.subagent), report))
        .collect::<BTreeMap<_, _>>();

    let off_off = score_for(&by_axes, false, false);
    let on_off = score_for(&by_axes, true, false);
    let off_on = score_for(&by_axes, false, true);
    let on_on = score_for(&by_axes, true, true);

    DeNovoComparisonReport {
        conditions: reports,
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
    conditions: &BTreeMap<(bool, bool), &DeNovoConditionReport>,
    stateful: bool,
    subagent: bool,
) -> Option<f64> {
    conditions
        .get(&(stateful, subagent))
        .and_then(|report| report.average_score)
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
