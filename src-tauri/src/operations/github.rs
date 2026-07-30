use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use reqwest::{header::LOCATION, Client, Response, StatusCode, Url};
use serde::Deserialize;
use tokio::time::timeout;
use zip::ZipArchive;

use super::{
    copy_source_to_staging, inspect_source, path_exists, random_token, valid_operation_id,
    validate_import_metadata, ImportSourceKind, ManagementOperation, OperationError,
    OperationImpact, OperationPlan, OperationPlanStatus, OperationSource, OperationsService,
    PendingImportSource, PlannedImport,
};

const API_RESPONSE_LIMIT: usize = 1024 * 1024;
const ARCHIVE_RESPONSE_LIMIT: usize = 32 * 1024 * 1024;
const ARCHIVE_ENTRY_LIMIT: usize = 512;
const ARCHIVE_FILE_LIMIT: u64 = 16 * 1024 * 1024;
const ARCHIVE_EXTRACTED_LIMIT: u64 = 32 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PLAN_TIMEOUT: Duration = Duration::from_secs(45);

pub(super) fn cleanup_abandoned_staging(root: &Path) {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if valid_operation_id(&name) && file_type.is_dir() && !file_type.is_symlink() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GithubSource {
    owner: String,
    repository: String,
    repository_url: String,
    reference: String,
    subdirectory: String,
}

#[derive(Clone)]
pub(super) struct GithubEndpoints {
    api_base: Url,
    archive_base: Url,
    test_origin: Option<String>,
}

impl GithubEndpoints {
    fn production() -> Self {
        Self {
            api_base: Url::parse("https://api.github.com/").expect("static GitHub API URL"),
            archive_base: Url::parse("https://codeload.github.com/")
                .expect("static GitHub codeload URL"),
            test_origin: None,
        }
    }

    #[cfg(test)]
    pub(super) fn loopback(origin: &str) -> Self {
        let mut base = Url::parse(origin).expect("valid loopback fixture origin");
        base.set_path("/");
        base.set_query(None);
        base.set_fragment(None);
        let origin = origin_of(&base);
        Self {
            api_base: base.clone(),
            archive_base: base,
            test_origin: Some(origin),
        }
    }

    fn allows(&self, url: &Url) -> bool {
        if self
            .test_origin
            .as_ref()
            .is_some_and(|origin| origin == &origin_of(url))
        {
            return url.host_str().is_some_and(is_loopback_host);
        }
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && matches!(
                url.host_str(),
                Some("api.github.com" | "github.com" | "codeload.github.com")
            )
    }
}

#[derive(Deserialize)]
struct CommitResponse {
    sha: String,
}

impl OperationError {
    const fn github_source_invalid() -> Self {
        Self {
            code: "github_source_invalid",
            message: "The GitHub source is not valid.",
            recovery: "Use a public https://github.com/owner/repository URL and valid ref.",
        }
    }

    const fn github_repository_not_found() -> Self {
        Self {
            code: "github_repository_not_found",
            message: "The public GitHub repository is unavailable.",
            recovery: "Check the repository URL and confirm that it is public.",
        }
    }

    const fn github_ref_not_found() -> Self {
        Self {
            code: "github_ref_not_found",
            message: "The GitHub ref could not be resolved.",
            recovery: "Check the branch, tag, or commit and review the source again.",
        }
    }

    const fn github_rate_limited() -> Self {
        Self {
            code: "github_rate_limited",
            message: "GitHub temporarily limited this request.",
            recovery: "Wait before reviewing the GitHub import again.",
        }
    }

    const fn github_offline() -> Self {
        Self {
            code: "github_offline",
            message: "GitHub could not be reached.",
            recovery: "Check the network connection and try again.",
        }
    }

    const fn github_timeout() -> Self {
        Self {
            code: "github_timeout",
            message: "The GitHub import request timed out.",
            recovery: "Check the network connection and review the import again.",
        }
    }

    const fn github_response_too_large() -> Self {
        Self {
            code: "github_response_too_large",
            message: "The GitHub response exceeds the safe import limit.",
            recovery: "Choose a smaller repository or a narrower Skill source.",
        }
    }

    const fn github_protocol_error() -> Self {
        Self {
            code: "github_protocol_error",
            message: "GitHub returned an unsupported response.",
            recovery: "Review the repository source again later.",
        }
    }

    const fn github_archive_invalid() -> Self {
        Self {
            code: "github_archive_invalid",
            message: "The GitHub archive cannot be extracted safely.",
            recovery: "Check the repository archive structure and try again.",
        }
    }

    const fn github_skill_not_found() -> Self {
        Self {
            code: "github_skill_not_found",
            message: "No importable Skill was found at the selected location.",
            recovery: "Choose the repository subdirectory that contains SKILL.md.",
        }
    }

    const fn github_multiple_skills() -> Self {
        Self {
            code: "github_multiple_skills",
            message: "The repository contains multiple Skill candidates.",
            recovery: "Enter one Skill subdirectory before reviewing the import.",
        }
    }
}

impl OperationsService {
    pub(super) async fn plan_github_import(
        &self,
        repository_url: &str,
        reference: &str,
        subdirectory: &str,
    ) -> Result<PlannedImport, OperationError> {
        self.plan_github_import_with_endpoints(
            repository_url,
            reference,
            subdirectory,
            GithubEndpoints::production(),
        )
        .await
    }

    #[cfg(test)]
    pub(super) async fn plan_github_import_for_test(
        &self,
        repository_url: &str,
        reference: &str,
        subdirectory: &str,
        endpoints: GithubEndpoints,
    ) -> Result<PlannedImport, OperationError> {
        self.plan_github_import_with_endpoints(repository_url, reference, subdirectory, endpoints)
            .await
    }

    async fn plan_github_import_with_endpoints(
        &self,
        repository_url: &str,
        reference: &str,
        subdirectory: &str,
        endpoints: GithubEndpoints,
    ) -> Result<PlannedImport, OperationError> {
        let source = parse_source(repository_url, reference, subdirectory)?;
        let operation_id = random_token().ok_or_else(OperationError::selection_unavailable)?;
        let operation_root = self.create_github_operation_root(&operation_id)?;
        let prepared = match timeout(
            PLAN_TIMEOUT,
            self.prepare_github_plan(&source, &operation_id, &operation_root, &endpoints),
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(_) => {
                let _ = fs::remove_dir_all(&operation_root);
                return Err(OperationError::github_timeout());
            }
        };
        let (plan, pending) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = fs::remove_dir_all(&operation_root);
                return Err(error);
            }
        };
        self.finalize_remote_plan(plan, pending, &operation_root)
    }

    async fn prepare_github_plan(
        &self,
        source: &GithubSource,
        operation_id: &str,
        operation_root: &Path,
        endpoints: &GithubEndpoints,
    ) -> Result<(OperationPlan, PendingImportSource), OperationError> {
        let client = github_client()?;
        let commit_sha = resolve_commit(&client, source, endpoints).await?;
        let archive = download_archive(&client, source, &commit_sha, endpoints).await?;
        let repository_root = extract_archive(&archive, operation_root)?;
        let selected = select_skill_directory(&repository_root, &source.subdirectory)?;
        let selected_summary = inspect_source(ImportSourceKind::Directory, &selected)
            .map_err(|_| OperationError::github_skill_not_found())?;
        let staging_home = operation_root.join("home");
        let staged_skill = staging_home
            .join(".agents")
            .join("skills")
            .join(&selected_summary.target_name);
        copy_source_to_staging(&selected_summary, &staged_skill)?;
        let staged = inspect_source(ImportSourceKind::Directory, &staged_skill)?;
        if staged.source_hash != selected_summary.source_hash {
            return Err(OperationError::source_changed());
        }
        let facts = self
            .catalog
            .validate_import_staging(staging_home.clone(), &staged.target_name)
            .map_err(|_| OperationError::import_source_invalid())?;
        validate_import_metadata(&facts.name, &facts.description, &staged.target_name)?;
        let provenance = OperationSource {
            source_type: "github".to_owned(),
            repository_url: source.repository_url.clone(),
            repo_ref: source.reference.clone(),
            commit_sha,
            subdirectory: source.subdirectory.clone(),
        };
        let conflict = path_exists(&self.target_root.join(&staged.target_name));
        let plan = OperationPlan {
            id: operation_id.to_owned(),
            operation: ManagementOperation::SkillImport,
            status: if conflict {
                OperationPlanStatus::Conflict
            } else {
                OperationPlanStatus::Ready
            },
            impact: OperationImpact {
                target_provider_id: "user_global".to_owned(),
                skill_name: staged.target_name.clone(),
                file_count: staged.file_count,
                total_size_bytes: staged.total_size_bytes,
                relative_files: staged
                    .files
                    .iter()
                    .filter_map(|(relative, _)| relative.to_str().map(str::to_owned))
                    .collect(),
                entry_id: None,
                requires_acknowledgement: false,
            },
            source: Some(provenance.clone()),
        };
        Ok((
            plan,
            PendingImportSource::Remote {
                operation_root: operation_root.to_path_buf(),
                staging_home,
                staged_skill,
                source_hash: staged.source_hash,
                provenance,
            },
        ))
    }

    pub(super) fn create_github_operation_root(
        &self,
        operation_id: &str,
    ) -> Result<PathBuf, OperationError> {
        let root = self
            .github_staging_root
            .as_deref()
            .ok_or_else(OperationError::quarantine_unavailable)?;
        fs::create_dir_all(root).map_err(|_| OperationError::quarantine_unavailable())?;
        let metadata =
            fs::symlink_metadata(root).map_err(|_| OperationError::quarantine_unavailable())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(OperationError::quarantine_unavailable());
        }
        let operation_root = root.join(operation_id);
        if operation_root.parent() != Some(root) {
            return Err(OperationError::github_archive_invalid());
        }
        fs::create_dir(&operation_root).map_err(|_| OperationError::import_failed())?;
        Ok(operation_root)
    }

    pub(super) fn validate_github_staging(
        &self,
        operation_root: &Path,
        staging_home: &Path,
        staged_skill: &Path,
    ) -> Result<(), OperationError> {
        let root = self
            .github_staging_root
            .as_deref()
            .ok_or_else(OperationError::quarantine_unavailable)?;
        for path in [root, operation_root, staging_home, staged_skill] {
            let metadata =
                fs::symlink_metadata(path).map_err(|_| OperationError::source_changed())?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(OperationError::source_changed());
            }
        }
        let canonical_root =
            fs::canonicalize(root).map_err(|_| OperationError::source_changed())?;
        let canonical_operation =
            fs::canonicalize(operation_root).map_err(|_| OperationError::source_changed())?;
        let canonical_home =
            fs::canonicalize(staging_home).map_err(|_| OperationError::source_changed())?;
        let canonical_skill =
            fs::canonicalize(staged_skill).map_err(|_| OperationError::source_changed())?;
        if canonical_operation.parent() != Some(canonical_root.as_path())
            || !canonical_home.starts_with(&canonical_operation)
            || !canonical_skill.starts_with(&canonical_home)
        {
            return Err(OperationError::source_changed());
        }
        Ok(())
    }
}

pub(super) fn validate_remote_provenance(source: &OperationSource) -> Result<(), OperationError> {
    let valid = match source.source_type.as_str() {
        "github" => parse_source(
            &source.repository_url,
            &source.repo_ref,
            &source.subdirectory,
        )
        .is_ok(),
        "market" => {
            let segments = source.subdirectory.split('/').collect::<Vec<_>>();
            source.repository_url == "https://github.com/openai/plugins"
                && source.repo_ref == "main"
                && segments.len() == 4
                && segments[0] == "plugins"
                && !segments[1].is_empty()
                && segments[2] == "skills"
                && !segments[3].is_empty()
                && valid_subdirectory(&source.subdirectory)
        }
        _ => false,
    };
    if !valid_commit_sha(&source.commit_sha) || !valid {
        return Err(OperationError::source_changed());
    }
    Ok(())
}

fn parse_source(
    repository_url: &str,
    reference: &str,
    subdirectory: &str,
) -> Result<GithubSource, OperationError> {
    if repository_url.len() > 512 || reference.len() > 255 || subdirectory.len() > 1024 {
        return Err(OperationError::github_source_invalid());
    }
    let authority = repository_url
        .strip_prefix("https://")
        .and_then(|value| value.split('/').next())
        .ok_or_else(OperationError::github_source_invalid)?;
    if authority.contains(':') {
        return Err(OperationError::github_source_invalid());
    }
    let url = Url::parse(repository_url).map_err(|_| OperationError::github_source_invalid())?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains("//")
    {
        return Err(OperationError::github_source_invalid());
    }
    let segments = url
        .path_segments()
        .ok_or_else(OperationError::github_source_invalid)?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() != 2 {
        return Err(OperationError::github_source_invalid());
    }
    let owner = segments[0].to_ascii_lowercase();
    let mut repository = segments[1].to_ascii_lowercase();
    if repository.ends_with(".git") {
        repository.truncate(repository.len() - 4);
    }
    if !valid_owner(&owner) || !valid_repository(&repository) {
        return Err(OperationError::github_source_invalid());
    }
    let reference = reference.trim();
    if !valid_ref(reference) {
        return Err(OperationError::github_source_invalid());
    }
    let subdirectory = subdirectory.trim();
    if !valid_subdirectory(subdirectory) {
        return Err(OperationError::github_source_invalid());
    }
    Ok(GithubSource {
        owner: owner.clone(),
        repository: repository.clone(),
        repository_url: format!("https://github.com/{owner}/{repository}"),
        reference: reference.to_owned(),
        subdirectory: subdirectory.to_owned(),
    })
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 39
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_repository(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && !value.contains("..")
        && !value.ends_with('.')
        && !value.ends_with(".lock")
        && !value.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'\\' | b'~' | b'^' | b':' | b'?' | b'*' | b'[')
        })
        && value
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn valid_subdirectory(value: &str) -> bool {
    value.is_empty()
        || (!value.starts_with('/')
            && !value.ends_with('/')
            && !value.contains("//")
            && !value
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == b'\\')
            && value.split('/').all(|segment| {
                !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(':')
            }))
}

fn valid_commit_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn github_client() -> Result<Client, OperationError> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent("Codex-O/1.0")
        .build()
        .map_err(|_| OperationError::github_protocol_error())
}

async fn resolve_commit(
    client: &Client,
    source: &GithubSource,
    endpoints: &GithubEndpoints,
) -> Result<String, OperationError> {
    let repository_url = endpoint_url(
        &endpoints.api_base,
        &["repos", &source.owner, &source.repository],
    )?;
    let repository_response = request_with_redirects(client, repository_url, endpoints).await?;
    ensure_status(
        repository_response.status(),
        OperationError::github_repository_not_found(),
    )?;
    let _ = read_limited(repository_response, API_RESPONSE_LIMIT).await?;

    let commit_url = endpoint_url(
        &endpoints.api_base,
        &[
            "repos",
            &source.owner,
            &source.repository,
            "commits",
            &source.reference,
        ],
    )?;
    let commit_response = request_with_redirects(client, commit_url, endpoints).await?;
    ensure_status(
        commit_response.status(),
        OperationError::github_ref_not_found(),
    )?;
    let body = read_limited(commit_response, API_RESPONSE_LIMIT).await?;
    let commit: CommitResponse =
        serde_json::from_slice(&body).map_err(|_| OperationError::github_protocol_error())?;
    let sha = commit.sha.to_ascii_lowercase();
    if !valid_commit_sha(&sha) {
        return Err(OperationError::github_protocol_error());
    }
    Ok(sha)
}

async fn download_archive(
    client: &Client,
    source: &GithubSource,
    commit_sha: &str,
    endpoints: &GithubEndpoints,
) -> Result<Vec<u8>, OperationError> {
    let archive_url = endpoint_url(
        &endpoints.archive_base,
        &[&source.owner, &source.repository, "zip", commit_sha],
    )?;
    let response = request_with_redirects(client, archive_url, endpoints).await?;
    ensure_status(response.status(), OperationError::github_protocol_error())?;
    read_limited(response, ARCHIVE_RESPONSE_LIMIT).await
}

fn endpoint_url(base: &Url, segments: &[&str]) -> Result<Url, OperationError> {
    let mut url = base.clone();
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| OperationError::github_protocol_error())?;
        path.clear();
        path.extend(segments.iter().copied());
    }
    Ok(url)
}

async fn request_with_redirects(
    client: &Client,
    mut url: Url,
    endpoints: &GithubEndpoints,
) -> Result<Response, OperationError> {
    for redirect_count in 0..=MAX_REDIRECTS {
        if !endpoints.allows(&url) {
            return Err(OperationError::github_protocol_error());
        }
        let response = client
            .get(url.clone())
            .send()
            .await
            .map_err(map_request_error)?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        if redirect_count == MAX_REDIRECTS {
            return Err(OperationError::github_protocol_error());
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(OperationError::github_protocol_error)?;
        url = url
            .join(location)
            .map_err(|_| OperationError::github_protocol_error())?;
    }
    Err(OperationError::github_protocol_error())
}

fn map_request_error(error: reqwest::Error) -> OperationError {
    if error.is_timeout() {
        OperationError::github_timeout()
    } else {
        OperationError::github_offline()
    }
}

fn ensure_status(status: StatusCode, not_found: OperationError) -> Result<(), OperationError> {
    if status.is_success() {
        Ok(())
    } else if status == StatusCode::NOT_FOUND || status == StatusCode::UNAUTHORIZED {
        Err(not_found)
    } else if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::FORBIDDEN {
        Err(OperationError::github_rate_limited())
    } else {
        Err(OperationError::github_protocol_error())
    }
}

async fn read_limited(mut response: Response, limit: usize) -> Result<Vec<u8>, OperationError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(OperationError::github_response_too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_request_error)? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(OperationError::github_response_too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn extract_archive(bytes: &[u8], operation_root: &Path) -> Result<PathBuf, OperationError> {
    let extraction_root = operation_root.join("archive");
    fs::create_dir(&extraction_root).map_err(|_| OperationError::github_archive_invalid())?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| OperationError::github_archive_invalid())?;
    if archive.is_empty() || archive.len() > ARCHIVE_ENTRY_LIMIT {
        return Err(OperationError::github_archive_invalid());
    }
    let mut paths = HashSet::new();
    let mut top_level: Option<String> = None;
    let mut extracted_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| OperationError::github_archive_invalid())?;
        let (relative, normalized) = normalized_zip_path(entry.name(), entry.is_dir())?;
        if !paths.insert(normalized) {
            return Err(OperationError::github_archive_invalid());
        }
        let first = relative
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str())
            .ok_or_else(OperationError::github_archive_invalid)?;
        if top_level.as_deref().is_some_and(|top| top != first) {
            return Err(OperationError::github_archive_invalid());
        }
        top_level.get_or_insert_with(|| first.to_owned());
        validate_entry_mode(entry.unix_mode(), entry.is_dir())?;
        if entry.size() > ARCHIVE_FILE_LIMIT && !entry.is_dir() {
            return Err(OperationError::github_response_too_large());
        }
        let destination = extraction_root.join(&relative);
        // The filesystem may fold names that differ at the ZIP string level.
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(OperationError::github_archive_invalid());
        }
        if entry.is_dir() {
            fs::create_dir_all(&destination)
                .map_err(|_| OperationError::github_archive_invalid())?;
            continue;
        }
        extracted_bytes = extracted_bytes.saturating_add(entry.size());
        if extracted_bytes > ARCHIVE_EXTRACTED_LIMIT {
            return Err(OperationError::github_response_too_large());
        }
        let parent = destination
            .parent()
            .ok_or_else(OperationError::github_archive_invalid)?;
        fs::create_dir_all(parent).map_err(|_| OperationError::github_archive_invalid())?;
        let mut output =
            fs::File::create(&destination).map_err(|_| OperationError::github_archive_invalid())?;
        let copied = std::io::copy(
            &mut entry.by_ref().take(ARCHIVE_FILE_LIMIT + 1),
            &mut output,
        )
        .map_err(|_| OperationError::github_archive_invalid())?;
        if copied != entry.size() || copied > ARCHIVE_FILE_LIMIT {
            return Err(OperationError::github_archive_invalid());
        }
        output
            .flush()
            .map_err(|_| OperationError::github_archive_invalid())?;
    }
    let top_level = top_level.ok_or_else(OperationError::github_archive_invalid)?;
    let repository_root = extraction_root.join(top_level);
    let metadata = fs::symlink_metadata(&repository_root)
        .map_err(|_| OperationError::github_archive_invalid())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(OperationError::github_archive_invalid());
    }
    Ok(repository_root)
}

fn normalized_zip_path(
    name: &str,
    is_directory: bool,
) -> Result<(PathBuf, String), OperationError> {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(OperationError::github_archive_invalid());
    }
    let trimmed = if is_directory {
        name.strip_suffix('/').unwrap_or(name)
    } else {
        name
    };
    let segments = trimmed.split('/').collect::<Vec<_>>();
    if segments.is_empty()
        || segments.iter().any(|segment| {
            segment.is_empty() || *segment == "." || *segment == ".." || segment.contains(':')
        })
    {
        return Err(OperationError::github_archive_invalid());
    }
    let normalized = segments.join("/");
    Ok((segments.iter().collect(), normalized))
}

fn validate_entry_mode(mode: Option<u32>, is_directory: bool) -> Result<(), OperationError> {
    let Some(mode) = mode else {
        return Ok(());
    };
    let file_type = mode & 0o170000;
    let allowed = if is_directory {
        file_type == 0 || file_type == 0o040000
    } else {
        file_type == 0 || file_type == 0o100000
    };
    if allowed {
        Ok(())
    } else {
        Err(OperationError::github_archive_invalid())
    }
}

fn select_skill_directory(
    repository_root: &Path,
    subdirectory: &str,
) -> Result<PathBuf, OperationError> {
    if !subdirectory.is_empty() {
        let selected = repository_root.join(subdirectory);
        let metadata = fs::symlink_metadata(&selected)
            .map_err(|_| OperationError::github_skill_not_found())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || !selected.join(super::SKILL_MARKDOWN_FILE).is_file()
        {
            return Err(OperationError::github_skill_not_found());
        }
        return Ok(selected);
    }
    let mut candidates = Vec::new();
    collect_skill_candidates(repository_root, &mut candidates)?;
    match candidates.len() {
        0 => Err(OperationError::github_skill_not_found()),
        1 => Ok(candidates.remove(0)),
        _ => Err(OperationError::github_multiple_skills()),
    }
}

fn collect_skill_candidates(
    directory: &Path,
    candidates: &mut Vec<PathBuf>,
) -> Result<(), OperationError> {
    let metadata =
        fs::symlink_metadata(directory).map_err(|_| OperationError::github_archive_invalid())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(OperationError::github_archive_invalid());
    }
    if directory.join(super::SKILL_MARKDOWN_FILE).is_file() {
        candidates.push(directory.to_path_buf());
        if candidates.len() > 1 {
            return Ok(());
        }
    }
    for entry in fs::read_dir(directory).map_err(|_| OperationError::github_archive_invalid())? {
        let entry = entry.map_err(|_| OperationError::github_archive_invalid())?;
        let file_type = entry
            .file_type()
            .map_err(|_| OperationError::github_archive_invalid())?;
        if file_type.is_symlink() {
            return Err(OperationError::github_archive_invalid());
        }
        if file_type.is_dir() {
            collect_skill_candidates(&entry.path(), candidates)?;
            if candidates.len() > 1 {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn origin_of(url: &Url) -> String {
    format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        url.port_or_known_default().unwrap_or_default()
    )
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "[::1]" | "::1" | "localhost")
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Duration,
    };

    use rusqlite::Connection;
    use tempfile::TempDir;
    use zip::{write::SimpleFileOptions, ZipWriter};

    use crate::{
        catalog::SkillCatalog, db, observability::DiagnosticService, providers::ProviderRoots,
    };

    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    struct TestServer {
        origin: String,
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn start(handler: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = Arc::clone(&stop);
            let handler = Arc::new(handler);
            let thread = thread::spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => handle_connection(&mut stream, &handler),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                origin,
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    fn handle_connection(
        stream: &mut TcpStream,
        handler: &Arc<impl Fn(&str) -> Vec<u8> + Send + Sync + 'static>,
    ) {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::with_capacity(1024);
        let mut chunk = [0_u8; 1024];
        // TCP may split the request line across reads, especially when tests run in parallel.
        while request.len() < 8192 {
            let size = stream.read(&mut chunk).unwrap_or(0);
            if size == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..size]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let first_line = String::from_utf8_lossy(&request)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        let path = first_line.split_whitespace().nth(1).unwrap_or("/");
        let response = handler(path);
        let _ = stream.write_all(&response);
        let _ = stream.flush();
    }

    fn response(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        response
            .into_bytes()
            .into_iter()
            .chain(body.iter().copied())
            .collect()
    }

    fn zip_files(entries: &[(&str, &str)]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        for (name, content) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn valid_skill(name: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: A safe GitHub fixture.\n---\n# Overview\nFixture body.\n"
        )
    }

    fn operation_root(temporary: &TempDir) -> PathBuf {
        let root = temporary.path().join("operation");
        fs::create_dir(&root).unwrap();
        root
    }

    struct ServiceFixture {
        temporary: TempDir,
        home: PathBuf,
        database_path: PathBuf,
        service: OperationsService,
    }

    impl ServiceFixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let home = temporary.path().join("home");
            let database_path = temporary.path().join("data.db");
            assert!(matches!(
                db::initialize(database_path.clone()),
                db::AppDatabase::Ready(_)
            ));
            let roots = ProviderRoots::new(
                home.clone(),
                temporary.path().join("repository"),
                temporary.path().join("plugin-cache"),
            );
            let service = OperationsService::new(
                Some(database_path.clone()),
                Some(temporary.path().join("app-local")),
                SkillCatalog::with_index_path(roots, database_path.clone()),
                DiagnosticService::new(None, None),
            );
            Self {
                temporary,
                home,
                database_path,
                service,
            }
        }

        fn runtime(&self) -> tokio::runtime::Runtime {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        }
    }

    fn github_server(archive: Vec<u8>) -> TestServer {
        TestServer::start(move |path| {
            if path.contains("/commits/") {
                response(
                    "200 OK",
                    &[("Content-Type", "application/json".to_owned())],
                    format!("{{\"sha\":\"{SHA}\"}}").as_bytes(),
                )
            } else if path.contains("/zip/") {
                response("200 OK", &[], &archive)
            } else {
                response("200 OK", &[], b"{}")
            }
        })
    }

    #[test]
    fn source_parser_normalizes_repository_url() {
        let source = parse_source("https://github.com/OpenAI/Codex.GIT/", "main", "").unwrap();

        assert_eq!(source.owner, "openai");
        assert_eq!(source.repository, "codex");
        assert_eq!(source.repository_url, "https://github.com/openai/codex");
    }

    #[test]
    fn source_parser_rejects_non_github_and_ambiguous_urls() {
        for url in [
            "http://github.com/openai/codex",
            "https://example.com/openai/codex",
            "https://user@github.com/openai/codex",
            "https://github.com:443/openai/codex",
            "https://github.com:444/openai/codex",
            "https://github.com/openai/codex?tab=readme",
            "https://github.com/openai/codex/tree/main",
        ] {
            assert_eq!(
                parse_source(url, "main", "").unwrap_err().code,
                "github_source_invalid"
            );
        }
    }

    #[test]
    fn market_provenance_requires_the_official_four_segment_skill_path() {
        let valid = OperationSource {
            source_type: "market".to_owned(),
            repository_url: "https://github.com/openai/plugins".to_owned(),
            repo_ref: "main".to_owned(),
            commit_sha: SHA.to_owned(),
            subdirectory: "plugins/example/skills/reviewer".to_owned(),
        };
        assert!(validate_remote_provenance(&valid).is_ok());

        for subdirectory in [
            "plugins/example/reviewer",
            "plugins/example/other/skills/reviewer",
            "plugins/example/skills/reviewer/extra",
        ] {
            assert_eq!(
                validate_remote_provenance(&OperationSource {
                    subdirectory: subdirectory.to_owned(),
                    ..valid.clone()
                })
                .unwrap_err()
                .code,
                "source_changed"
            );
        }
    }

    #[test]
    fn source_parser_accepts_slashes_in_ref_and_subdirectory() {
        let source = parse_source(
            "https://github.com/openai/codex",
            "feature/install/github",
            "skills/reviewer",
        )
        .unwrap();

        assert_eq!(source.reference, "feature/install/github");
        assert_eq!(source.subdirectory, "skills/reviewer");
    }

    #[test]
    fn source_parser_rejects_ref_traversal_and_control_characters() {
        for reference in [
            "", "/main", "main/", "a//b", "a/../b", "bad ref", "bad\\ref",
        ] {
            assert_eq!(
                parse_source("https://github.com/openai/codex", reference, "")
                    .unwrap_err()
                    .code,
                "github_source_invalid"
            );
        }
    }

    #[test]
    fn source_parser_rejects_subdirectory_traversal() {
        for subdirectory in ["/skills/a", "skills/a/", "skills//a", "skills/../a", "a\\b"] {
            assert_eq!(
                parse_source("https://github.com/openai/codex", "main", subdirectory)
                    .unwrap_err()
                    .code,
                "github_source_invalid"
            );
        }
    }

    #[test]
    fn commit_sha_requires_exactly_forty_hex_characters() {
        assert!(valid_commit_sha(SHA));
        assert!(!valid_commit_sha("abc"));
        assert!(!valid_commit_sha(
            "g123456789abcdef0123456789abcdef01234567"
        ));
    }

    #[test]
    fn valid_archive_extracts_selected_skill() {
        let temporary = tempfile::tempdir().unwrap();
        let archive = zip_files(&[("repo-sha/skills/demo/SKILL.md", &valid_skill("demo"))]);
        let repository = extract_archive(&archive, &operation_root(&temporary)).unwrap();

        assert_eq!(
            select_skill_directory(&repository, "skills/demo").unwrap(),
            repository.join("skills/demo")
        );
    }

    #[test]
    fn zip_slip_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let operation_root = operation_root(&temporary);
        let archive = zip_files(&[("repo-sha/../../escape", "unsafe")]);

        let error = extract_archive(&archive, &operation_root).unwrap_err();

        assert_eq!(error.code, "github_archive_invalid");
        assert!(!operation_root.join("escape").exists());
    }

    #[test]
    fn symlink_archive_entry_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        writer
            .add_symlink("repo-sha/link", "../outside", SimpleFileOptions::default())
            .unwrap();
        let archive = writer.finish().unwrap().into_inner();

        assert_eq!(
            extract_archive(&archive, &operation_root(&temporary))
                .unwrap_err()
                .code,
            "github_archive_invalid"
        );
    }

    #[test]
    fn duplicate_archive_path_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        writer
            .add_directory("repo-sha/a/", SimpleFileOptions::default())
            .unwrap();
        writer
            .start_file("repo-sha/a", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"file").unwrap();
        let archive = writer.finish().unwrap().into_inner();

        assert_eq!(
            extract_archive(&archive, &operation_root(&temporary))
                .unwrap_err()
                .code,
            "github_archive_invalid"
        );
    }

    #[test]
    fn repository_without_skill_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let archive = zip_files(&[("repo-sha/README.md", "readme")]);
        let repository = extract_archive(&archive, &operation_root(&temporary)).unwrap();

        assert_eq!(
            select_skill_directory(&repository, "").unwrap_err().code,
            "github_skill_not_found"
        );
    }

    #[test]
    fn repository_with_multiple_skills_requires_subdirectory() {
        let temporary = tempfile::tempdir().unwrap();
        let first = valid_skill("first");
        let second = valid_skill("second");
        let archive = zip_files(&[
            ("repo-sha/skills/first/SKILL.md", &first),
            ("repo-sha/skills/second/SKILL.md", &second),
        ]);
        let repository = extract_archive(&archive, &operation_root(&temporary)).unwrap();

        assert_eq!(
            select_skill_directory(&repository, "").unwrap_err().code,
            "github_multiple_skills"
        );
    }

    #[test]
    fn disallowed_redirect_host_is_rejected() {
        let server = TestServer::start(|_| {
            response(
                "302 Found",
                &[("Location", "https://example.com/private".to_owned())],
                b"",
            )
        });
        let endpoints = GithubEndpoints::loopback(&server.origin);
        let client = github_client().unwrap();
        let source = parse_source("https://github.com/openai/codex", "main", "").unwrap();

        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(resolve_commit(&client, &source, &endpoints))
            .unwrap_err();

        assert_eq!(error.code, "github_protocol_error");
    }

    #[test]
    fn same_origin_redirect_is_followed() {
        let server = TestServer::start(|path| {
            if path == "/repos/openai/codex" {
                response(
                    "302 Found",
                    &[("Location", "/repository-ok".to_owned())],
                    b"",
                )
            } else if path == "/repository-ok" {
                response("200 OK", &[], b"{}")
            } else {
                response(
                    "200 OK",
                    &[("Content-Type", "application/json".to_owned())],
                    format!("{{\"sha\":\"{SHA}\"}}").as_bytes(),
                )
            }
        });
        let endpoints = GithubEndpoints::loopback(&server.origin);
        let client = github_client().unwrap();
        let source = parse_source("https://github.com/openai/codex", "main", "").unwrap();

        let sha = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(resolve_commit(&client, &source, &endpoints))
            .unwrap();

        assert_eq!(sha, SHA);
    }

    #[test]
    fn missing_ref_has_stable_error_code() {
        let server = TestServer::start(|path| {
            if path.contains("/commits/") {
                response("404 Not Found", &[], b"private detail")
            } else {
                response("200 OK", &[], b"{}")
            }
        });
        let endpoints = GithubEndpoints::loopback(&server.origin);
        let client = github_client().unwrap();
        let source = parse_source("https://github.com/openai/codex", "missing", "").unwrap();

        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(resolve_commit(&client, &source, &endpoints))
            .unwrap_err();

        assert_eq!(error.code, "github_ref_not_found");
        assert!(!error.message.contains("private detail"));
    }

    #[test]
    fn connection_failure_is_mapped_to_offline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let endpoints = GithubEndpoints::loopback(&origin);
        let client = github_client().unwrap();
        let source = parse_source("https://github.com/openai/codex", "main", "").unwrap();

        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(resolve_commit(&client, &source, &endpoints))
            .unwrap_err();

        assert_eq!(error.code, "github_offline");
    }

    #[test]
    fn rate_limit_is_mapped_without_response_body() {
        let server = TestServer::start(|_| response("429 Too Many Requests", &[], b"secret"));
        let endpoints = GithubEndpoints::loopback(&server.origin);
        let client = github_client().unwrap();
        let source = parse_source("https://github.com/openai/codex", "main", "").unwrap();

        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(resolve_commit(&client, &source, &endpoints))
            .unwrap_err();

        assert_eq!(error.code, "github_rate_limited");
        assert!(!error.message.contains("secret"));
    }

    #[test]
    fn oversized_response_is_rejected_from_headers() {
        let server = TestServer::start(|_| {
            let body = vec![b'x'; API_RESPONSE_LIMIT + 1];
            response("200 OK", &[], &body)
        });
        let endpoints = GithubEndpoints::loopback(&server.origin);
        let client = github_client().unwrap();
        let source = parse_source("https://github.com/openai/codex", "main", "").unwrap();

        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(resolve_commit(&client, &source, &endpoints))
            .unwrap_err();

        assert_eq!(error.code, "github_response_too_large");
    }

    #[test]
    fn github_plan_executes_and_persists_receipt_commit_sha() {
        let fixture = ServiceFixture::new();
        let skill = valid_skill("github-demo");
        let archive = zip_files(&[("repo-sha/skills/github-demo/SKILL.md", &skill)]);
        let server = github_server(archive);
        let plan = fixture
            .runtime()
            .block_on(fixture.service.plan_github_import_for_test(
                "https://github.com/OpenAI/Codex.git",
                "main",
                "skills/github-demo",
                GithubEndpoints::loopback(&server.origin),
            ))
            .unwrap();

        let result = fixture
            .service
            .execute_import(&plan.confirmation_token.unwrap().token)
            .unwrap();

        assert!(fixture.home.join(".agents/skills/github-demo").is_dir());
        let receipt: (String, String, String, String, String) =
            Connection::open(&fixture.database_path)
                .unwrap()
                .query_row(
                    "SELECT source_type, source_url, repo_ref, commit_sha, subdirectory FROM install_receipts WHERE skill_id = ?1",
                    [&result.skill_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .unwrap();
        assert_eq!(
            receipt,
            (
                "github".to_owned(),
                "https://github.com/openai/codex".to_owned(),
                "main".to_owned(),
                SHA.to_owned(),
                "skills/github-demo".to_owned(),
            )
        );
        assert!(!fixture
            .temporary
            .path()
            .join("app-local/github-staging")
            .join(plan.plan.id)
            .exists());
    }

    #[test]
    fn github_staging_change_is_rejected_before_install() {
        let fixture = ServiceFixture::new();
        let skill = valid_skill("github-changed");
        let archive = zip_files(&[("repo-sha/github-changed/SKILL.md", &skill)]);
        let server = github_server(archive);
        let plan = fixture
            .runtime()
            .block_on(fixture.service.plan_github_import_for_test(
                "https://github.com/openai/codex",
                "main",
                "github-changed",
                GithubEndpoints::loopback(&server.origin),
            ))
            .unwrap();
        fs::write(
            fixture
                .temporary
                .path()
                .join("app-local/github-staging")
                .join(&plan.plan.id)
                .join("home/.agents/skills/github-changed/SKILL.md"),
            valid_skill("github-changed") + "changed",
        )
        .unwrap();

        let error = fixture
            .service
            .execute_import(&plan.confirmation_token.unwrap().token)
            .unwrap_err();

        assert_eq!(error.code, "source_changed");
        assert!(!fixture.home.join(".agents/skills/github-changed").exists());
    }

    #[test]
    fn expired_github_confirmation_cleans_operation_staging() {
        let fixture = ServiceFixture::new();
        let skill = valid_skill("github-expired");
        let archive = zip_files(&[("repo-sha/github-expired/SKILL.md", &skill)]);
        let server = github_server(archive);
        let plan = fixture
            .runtime()
            .block_on(fixture.service.plan_github_import_for_test(
                "https://github.com/openai/codex",
                "main",
                "github-expired",
                GithubEndpoints::loopback(&server.origin),
            ))
            .unwrap();
        let token = plan.confirmation_token.unwrap().token;
        fixture.service.expire_confirmation(&token);

        let error = fixture.service.execute_import(&token).unwrap_err();

        assert_eq!(error.code, "confirmation_token_expired");
        assert!(!fixture
            .temporary
            .path()
            .join("app-local/github-staging")
            .join(plan.plan.id)
            .exists());
    }

    #[test]
    fn cancelled_github_confirmation_cleans_operation_staging() {
        let fixture = ServiceFixture::new();
        let skill = valid_skill("github-cancelled");
        let archive = zip_files(&[("repo-sha/github-cancelled/SKILL.md", &skill)]);
        let server = github_server(archive);
        let plan = fixture
            .runtime()
            .block_on(fixture.service.plan_github_import_for_test(
                "https://github.com/openai/codex",
                "main",
                "github-cancelled",
                GithubEndpoints::loopback(&server.origin),
            ))
            .unwrap();
        let operation_root = fixture
            .temporary
            .path()
            .join("app-local/github-staging")
            .join(&plan.plan.id);
        let token = plan.confirmation_token.unwrap().token;
        assert!(operation_root.is_dir());

        fixture.service.cancel_import(&token).unwrap();

        assert!(!operation_root.exists());
        assert_eq!(
            fixture.service.execute_import(&token).unwrap_err().code,
            "confirmation_token_invalid"
        );
    }

    #[test]
    fn service_startup_cleans_only_abandoned_operation_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let app_local = temporary.path().join("app-local");
        let staging_root = app_local.join("github-staging");
        let abandoned = staging_root.join("a".repeat(64));
        let unrelated = staging_root.join("manual-notes");
        fs::create_dir_all(&abandoned).unwrap();
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(abandoned.join("archive.zip"), b"stale").unwrap();
        fs::write(unrelated.join("keep.txt"), b"keep").unwrap();
        let database_path = temporary.path().join("data.db");
        assert!(matches!(
            db::initialize(database_path.clone()),
            db::AppDatabase::Ready(_)
        ));
        let roots = ProviderRoots::new(
            temporary.path().join("home"),
            temporary.path().join("repository"),
            temporary.path().join("plugin-cache"),
        );

        let _service = OperationsService::new(
            Some(database_path.clone()),
            Some(app_local),
            SkillCatalog::with_index_path(roots, database_path),
            DiagnosticService::new(None, None),
        );

        assert!(!abandoned.exists());
        assert!(unrelated.join("keep.txt").is_file());
    }

    #[test]
    fn github_conflict_plan_has_no_token_and_cleans_staging() {
        let fixture = ServiceFixture::new();
        fs::create_dir_all(fixture.home.join(".agents/skills/github-conflict")).unwrap();
        fs::write(
            fixture.home.join(".agents/skills/github-conflict/SKILL.md"),
            valid_skill("github-conflict"),
        )
        .unwrap();
        let skill = valid_skill("github-conflict");
        let archive = zip_files(&[("repo-sha/github-conflict/SKILL.md", &skill)]);
        let server = github_server(archive);

        let plan = fixture
            .runtime()
            .block_on(fixture.service.plan_github_import_for_test(
                "https://github.com/openai/codex",
                "main",
                "github-conflict",
                GithubEndpoints::loopback(&server.origin),
            ))
            .unwrap();

        assert_eq!(plan.plan.status, OperationPlanStatus::Conflict);
        assert!(plan.confirmation_token.is_none());
        assert!(!fixture
            .temporary
            .path()
            .join("app-local/github-staging")
            .join(plan.plan.id)
            .exists());
    }
}
