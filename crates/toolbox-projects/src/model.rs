use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectKind {
    Folder,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProjectTarget {
    LocalPath(PathBuf),
    Uri(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentProject {
    pub id: ProjectId,
    pub provider: ProviderId,
    pub kind: ProjectKind,
    pub target: ProjectTarget,
    pub name: String,
    pub detail: String,
    pub recent_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiagnostic {
    pub level: DiagnosticLevel,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReport<T> {
    pub value: T,
    pub diagnostics: Vec<ProviderDiagnostic>,
}

pub trait RecentProjectProvider {
    fn discover(&self) -> ProviderReport<Vec<RecentProject>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenOutcome {
    Focused,
    OpenedNew,
}

pub trait ProjectOpener {
    type Error;

    fn open_or_focus(&self, project: &RecentProject) -> Result<OpenOutcome, Self::Error>;
}
