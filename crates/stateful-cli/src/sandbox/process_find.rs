use serde_json::Value;
use std::{path::Path, process::Command};

use super::{SandboxProcessFindOutput, SandboxProcessFindRequest, SandboxProcessInfo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SandboxProcessRow {
    info: SandboxProcessInfo,
    command: String,
}

const PROCESS_FIND_DEFAULT_FIELDS: &[&str] = &[
    "pid", "ppid", "pgid", "user", "uid", "stat", "start", "etime", "time", "pcpu", "pmem", "rss",
    "vsz", "nice", "pri", "tty", "comm",
];

const PROCESS_FIND_FORBIDDEN_FIELDS: &[&str] = &["command", "args", "argv", "env"];

pub(crate) fn validate_process_find_request(
    request: &SandboxProcessFindRequest,
) -> anyhow::Result<()> {
    if request.names.is_empty()
        && request.contains.is_empty()
        && request.pids.is_empty()
        && request.parent_pids.is_empty()
        && request.process_groups.is_empty()
    {
        anyhow::bail!("stateful sandbox process find requires at least one selector");
    }
    for name in &request.names {
        validate_process_name_selector(name)?;
    }
    for contains in &request.contains {
        validate_process_contains_selector(contains)?;
    }
    for (label, ids) in [
        ("--pid", request.pids.as_slice()),
        ("--parent-pid", request.parent_pids.as_slice()),
        ("--process-group", request.process_groups.as_slice()),
    ] {
        if ids.contains(&0) {
            anyhow::bail!("stateful sandbox process find {label} selectors must be positive");
        }
    }

    validate_process_find_fields(&request.fields)?;

    Ok(())
}

pub fn run_sandbox_process_find(
    request: SandboxProcessFindRequest,
) -> anyhow::Result<SandboxProcessFindOutput> {
    validate_process_find_request(&request)?;
    let rows = read_process_find_rows()?;
    process_find_output_for_rows(&request, rows)
}

fn validate_process_name_selector(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("stateful sandbox process find --name must not be empty");
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        anyhow::bail!("stateful sandbox process find --name contains unsupported characters");
    }
    Ok(())
}

fn validate_process_contains_selector(contains: &str) -> anyhow::Result<()> {
    let trimmed = contains.trim();
    if trimmed.len() < 3 {
        anyhow::bail!("stateful sandbox process find --contains must be at least 3 characters");
    }
    if trimmed.chars().any(char::is_control) {
        anyhow::bail!("stateful sandbox process find --contains contains control characters");
    }
    Ok(())
}

fn read_process_find_rows() -> anyhow::Result<Vec<SandboxProcessRow>> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pgid=,user=,uid=,stat=,start=,etime=,time=,pcpu=,pmem=,rss=,vsz=,nice=,pri=,tty=,comm=,command="])
        .output()
        .or_else(|_| {
            Command::new("ps")
                .args(["-axo", "pid=,ppid=,pgid=,user=,uid=,stat=,start=,etime=,time=,pcpu=,pmem=,rss=,vsz=,nice=,pri=,tty=,comm=,command="])
                .output()
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "stateful sandbox process find failed to inspect processes: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    parse_process_find_ps_output(&String::from_utf8_lossy(&output.stdout))
}

pub(super) fn parse_process_find_ps_output(output: &str) -> anyhow::Result<Vec<SandboxProcessRow>> {
    let mut rows = Vec::new();
    for line in output.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < PROCESS_FIND_DEFAULT_FIELDS.len() {
            continue;
        }
        let pid = parse_process_find_u32_field(fields[0], "pid")?;
        let ppid = parse_process_find_u32_field(fields[1], "ppid")?;
        let pgid = parse_process_find_u32_field(fields[2], "pgid")?;
        let user = fields[3].to_string();
        let uid = parse_process_find_i32_field(fields[4], "uid")?;
        let stat = fields[5].to_string();
        let start = fields[6].to_string();
        let etime = fields[7].to_string();
        let time = fields[8].to_string();
        let pcpu = fields[9].to_string();
        let pmem = fields[10].to_string();
        let rss = parse_process_find_u64_field(fields[11], "rss")?;
        let vsz = parse_process_find_u64_field(fields[12], "vsz")?;
        let nice = parse_process_find_i32_field(fields[13], "nice")?;
        let pri = parse_process_find_i32_field(fields[14], "pri")?;
        let tty = fields[15].to_string();
        let comm = fields[16].to_string();
        let command = if fields.len() > PROCESS_FIND_DEFAULT_FIELDS.len() {
            fields[PROCESS_FIND_DEFAULT_FIELDS.len()..].join(" ")
        } else {
            comm.clone()
        };
        rows.push(SandboxProcessRow {
            info: SandboxProcessInfo {
                pid,
                ppid,
                pgid,
                user,
                uid,
                stat,
                start,
                etime,
                time,
                pcpu,
                pmem,
                rss,
                vsz,
                nice,
                pri,
                tty,
                comm,
            },
            command,
        });
    }
    Ok(rows)
}

fn parse_process_find_u32_field(value: &str, field: &str) -> anyhow::Result<u32> {
    value.parse::<u32>().map_err(|_| {
        anyhow::anyhow!("stateful sandbox process find invalid {field} field `{value}`")
    })
}

fn parse_process_find_u64_field(value: &str, field: &str) -> anyhow::Result<u64> {
    value.parse::<u64>().map_err(|_| {
        anyhow::anyhow!("stateful sandbox process find invalid {field} field `{value}`")
    })
}

fn parse_process_find_i32_field(value: &str, field: &str) -> anyhow::Result<i32> {
    value.parse::<i32>().map_err(|_| {
        anyhow::anyhow!("stateful sandbox process find invalid {field} field `{value}`")
    })
}

pub(super) fn filter_process_find_rows(
    request: &SandboxProcessFindRequest,
    rows: Vec<SandboxProcessRow>,
) -> Vec<SandboxProcessInfo> {
    rows.into_iter()
        .filter(|row| row.info.pid != std::process::id())
        .filter(|row| process_find_row_matches(request, row))
        .map(|row| row.info)
        .collect()
}

pub(super) fn process_find_output_for_rows(
    request: &SandboxProcessFindRequest,
    rows: Vec<SandboxProcessRow>,
) -> anyhow::Result<SandboxProcessFindOutput> {
    validate_process_find_fields(&request.fields)?;
    let processes = filter_process_find_rows(request, rows)
        .into_iter()
        .map(|process| process_find_info_to_json(&process, &request.fields))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(SandboxProcessFindOutput {
        status: "ok",
        processes,
    })
}

fn validate_process_find_fields(fields: &[String]) -> anyhow::Result<()> {
    for field in fields {
        if PROCESS_FIND_FORBIDDEN_FIELDS.contains(&field.as_str()) {
            anyhow::bail!("stateful sandbox process find cannot expose field `{field}`");
        }
        if !process_find_is_safe_field(field) {
            anyhow::bail!("stateful sandbox process find unknown field `{field}`");
        }
    }
    Ok(())
}

fn process_find_is_safe_field(field: &str) -> bool {
    PROCESS_FIND_DEFAULT_FIELDS.contains(&field)
}

fn process_find_info_to_json(
    info: &SandboxProcessInfo,
    requested_fields: &[String],
) -> anyhow::Result<Value> {
    let mut process = serde_json::Map::new();
    if requested_fields.is_empty() {
        for field in PROCESS_FIND_DEFAULT_FIELDS {
            process.insert(
                (*field).to_string(),
                process_find_info_field_value(info, field)?,
            );
        }
    } else {
        for field in requested_fields {
            process.insert(
                field.clone(),
                process_find_info_field_value(info, field.as_str())?,
            );
        }
    }
    Ok(Value::Object(process))
}

fn process_find_info_field_value(info: &SandboxProcessInfo, field: &str) -> anyhow::Result<Value> {
    match field {
        "pid" => Ok(Value::from(info.pid)),
        "ppid" => Ok(Value::from(info.ppid)),
        "pgid" => Ok(Value::from(info.pgid)),
        "user" => Ok(Value::from(info.user.clone())),
        "uid" => Ok(Value::from(info.uid)),
        "stat" => Ok(Value::from(info.stat.clone())),
        "start" => Ok(Value::from(info.start.clone())),
        "etime" => Ok(Value::from(info.etime.clone())),
        "time" => Ok(Value::from(info.time.clone())),
        "pcpu" => Ok(Value::from(info.pcpu.clone())),
        "pmem" => Ok(Value::from(info.pmem.clone())),
        "rss" => Ok(Value::from(info.rss)),
        "vsz" => Ok(Value::from(info.vsz)),
        "nice" => Ok(Value::from(info.nice)),
        "pri" => Ok(Value::from(info.pri)),
        "tty" => Ok(Value::from(info.tty.clone())),
        "comm" => Ok(Value::from(info.comm.clone())),
        _ if PROCESS_FIND_FORBIDDEN_FIELDS.contains(&field) => {
            anyhow::bail!("stateful sandbox process find cannot expose field `{field}`")
        }
        _ => anyhow::bail!("stateful sandbox process find unknown field `{field}`"),
    }
}

fn process_find_row_matches(request: &SandboxProcessFindRequest, row: &SandboxProcessRow) -> bool {
    (request.names.is_empty()
        || request
            .names
            .iter()
            .any(|name| row.info.comm == *name || process_comm_basename(&row.info.comm) == name))
        && (request.contains.is_empty()
            || request
                .contains
                .iter()
                .any(|contains| row.command.contains(contains)))
        && (request.pids.is_empty() || request.pids.contains(&row.info.pid))
        && (request.parent_pids.is_empty() || request.parent_pids.contains(&row.info.ppid))
        && (request.process_groups.is_empty() || request.process_groups.contains(&row.info.pgid))
}

pub(super) fn process_comm_basename(comm: &str) -> &str {
    Path::new(comm)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(comm)
}
