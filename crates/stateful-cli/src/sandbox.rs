use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SandboxFsProfile {
    ReadOnly,
    WriteTargets,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SandboxNetworkPolicy {
    Disabled,
    Enabled,
}
