use std::{
    fs,
    io::{Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use rusqlite::{params, Connection};
use tempfile::TempDir;
use zip::{write::SimpleFileOptions, ZipWriter};

use crate::{
    analysis::{
        AnalysisQueue, AnalysisService, NoopAnalysisProgressSink, UnavailableAnalysisCache,
    },
    catalog::SkillCatalog,
    db,
    market::MarketEndpoints,
    operations::{github::GithubEndpoints, OperationsService},
    providers::ProviderRoots,
};

use super::{changed_files, short_commit, SkillUpdateStatus, UpdateService};

const OLD_SHA: &str = "1111111111111111111111111111111111111111";
const NEW_SHA: &str = "2222222222222222222222222222222222222222";

struct Fixture {
    temporary: TempDir,
    database_path: PathBuf,
    app_local: PathBuf,
    catalog: SkillCatalog,
    operations: Arc<OperationsService>,
    analysis_queue: AnalysisQueue,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
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
        let catalog = SkillCatalog::with_index_path(roots, database_path.clone());
        let app_local = temporary.path().join("app-local");
        let analysis_service = Arc::new(AnalysisService::new(
            catalog.clone(),
            Arc::new(UnavailableAnalysisCache),
            None,
        ));
        let analysis_queue =
            AnalysisQueue::new(analysis_service, Arc::new(NoopAnalysisProgressSink));
        let operations = Arc::new(
            OperationsService::new(
                Some(database_path.clone()),
                Some(app_local.clone()),
                catalog.clone(),
            )
            .with_analysis_queue(analysis_queue.clone()),
        );
        Self {
            temporary,
            database_path,
            app_local,
            catalog,
            operations,
            analysis_queue,
        }
    }

    fn install(&self, name: &str, body: &str) -> String {
        let directory = self.temporary.path().join("home/.agents/skills").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("SKILL.md"), skill(name, body)).unwrap();
        let skill_id = self
            .catalog
            .scan_skills()
            .skills
            .into_iter()
            .find(|skill| skill.provider.id == "user_global" && skill.display_name == name)
            .unwrap()
            .id;
        let hash = self.catalog.current_content_hash(&skill_id).unwrap();
        Connection::open(&self.database_path)
            .unwrap()
            .execute(
                "INSERT INTO install_receipts(skill_id, source_type, source_url, repo_ref, commit_sha, subdirectory, installed_hash, installed_at, managed_by) VALUES(?1, 'github', 'https://github.com/openai/codex', 'main', ?2, ?3, ?4, 1, 'codex-o')",
                params![skill_id, OLD_SHA, format!("skills/{name}"), hash],
            )
            .unwrap();
        skill_id
    }

    fn write_repo_skill(&self, name: &str, body: &str) -> PathBuf {
        let directory = self
            .temporary
            .path()
            .join("repository/.agents/skills")
            .join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("SKILL.md"), skill(name, body)).unwrap();
        directory
    }

    fn update_receipt_source(
        &self,
        skill_id: &str,
        source_type: &str,
        source_url: &str,
        subdirectory: &str,
    ) {
        Connection::open(&self.database_path)
            .unwrap()
            .execute(
                "UPDATE install_receipts SET source_type = ?1, source_url = ?2, subdirectory = ?3 WHERE skill_id = ?4",
                params![source_type, source_url, subdirectory, skill_id],
            )
            .unwrap();
    }

    fn service(&self, server: &TestServer) -> UpdateService {
        self.service_at(&server.origin)
    }

    fn service_at(&self, origin: &str) -> UpdateService {
        UpdateService::with_endpoints(
            Arc::clone(&self.operations),
            self.catalog.clone(),
            GithubEndpoints::loopback(origin),
            MarketEndpoints::loopback(origin),
        )
    }

    fn runtime(&self) -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn target(&self, name: &str) -> PathBuf {
        self.temporary.path().join("home/.agents/skills").join(name)
    }
}

struct TestServer {
    origin: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn github(name: &str, body: &str) -> Self {
        let archive = zip_skill(name, body);
        Self::start(move |path| {
            if path.contains("/commits/") {
                response("200 OK", format!("{{\"sha\":\"{NEW_SHA}\"}}").as_bytes())
            } else if path.contains("/zip/") {
                response("200 OK", &archive)
            } else {
                response("200 OK", b"{}")
            }
        })
    }

    fn market(name: &str, body: &str) -> Self {
        let subdirectory = format!("plugins/alpha-plugin/skills/{name}");
        let archive = zip_path(
            &format!("repo-sha/{subdirectory}/SKILL.md"),
            &skill(name, body),
        );
        let skill_size = skill(name, body).len();
        // One server supplies the market snapshot and the fixed-commit archive used by planning.
        Self::start(move |path| {
            if path == "/repos/openai/plugins/commits/main" {
                response("200 OK", format!("{{\"sha\":\"{NEW_SHA}\"}}").as_bytes())
            } else if path == format!("/openai/plugins/{NEW_SHA}/.agents/plugins/marketplace.json")
            {
                response(
                    "200 OK",
                    b"{\"name\":\"openai-curated\",\"plugins\":[{\"name\":\"alpha-plugin\",\"source\":\"./plugins/alpha-plugin\",\"category\":\"Developer Tools\",\"description\":\"Fixture plugin.\",\"policy\":{\"installation\":\"AVAILABLE\",\"products\":[\"CODEX\"]}}]}",
                )
            } else if path == format!("/repos/openai/plugins/git/trees/{NEW_SHA}?recursive=1") {
                response(
                    "200 OK",
                    format!(
                        "{{\"sha\":\"{NEW_SHA}\",\"truncated\":false,\"tree\":[{{\"path\":\"{subdirectory}/SKILL.md\",\"mode\":\"100644\",\"type\":\"blob\",\"size\":{skill_size}}}]}}"
                    )
                    .as_bytes(),
                )
            } else if path.contains("/zip/") {
                response("200 OK", &archive)
            } else {
                response("404 Not Found", b"missing")
            }
        })
    }

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
    // Accepted sockets may inherit nonblocking mode on macOS; block until the request is complete.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
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
    let path = String::from_utf8_lossy(&request)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
    let _ = stream.write_all(&handler(&path));
}

fn response(status: &str, body: &[u8]) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes()
    .into_iter()
    .chain(body.iter().copied())
    .collect()
}

fn zip_skill(name: &str, body: &str) -> Vec<u8> {
    zip_path(
        &format!("repo-sha/skills/{name}/SKILL.md"),
        &skill(name, body),
    )
}

fn zip_path(path: &str, content: &str) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    writer
        .start_file(path, SimpleFileOptions::default())
        .unwrap();
    writer.write_all(content.as_bytes()).unwrap();
    writer.finish().unwrap().into_inner()
}

fn skill(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: Safe update fixture.\n---\n# Overview\n{body}\n")
}

fn write_tree(root: &Path, files: &[(&str, &str)]) {
    for (relative, content) in files {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

#[test]
fn commit_is_shortened_without_exposing_the_full_value() {
    assert_eq!(short_commit(NEW_SHA), "22222222");
}

#[test]
fn change_summary_detects_added_files() {
    let temporary = tempfile::tempdir().unwrap();
    let current = temporary.path().join("current");
    let remote = temporary.path().join("remote");
    write_tree(&current, &[("SKILL.md", "same")]);
    write_tree(&remote, &[("SKILL.md", "same"), ("references/a.md", "new")]);
    assert_eq!(
        changed_files(&current, &remote).unwrap(),
        vec!["references/a.md"]
    );
}

#[test]
fn change_summary_detects_modified_files() {
    let temporary = tempfile::tempdir().unwrap();
    let current = temporary.path().join("current");
    let remote = temporary.path().join("remote");
    write_tree(&current, &[("SKILL.md", "old")]);
    write_tree(&remote, &[("SKILL.md", "new")]);
    assert_eq!(changed_files(&current, &remote).unwrap(), vec!["SKILL.md"]);
}

#[test]
fn change_summary_detects_removed_files() {
    let temporary = tempfile::tempdir().unwrap();
    let current = temporary.path().join("current");
    let remote = temporary.path().join("remote");
    write_tree(
        &current,
        &[("SKILL.md", "same"), ("references/old.txt", "old")],
    );
    write_tree(&remote, &[("SKILL.md", "same")]);
    assert_eq!(
        changed_files(&current, &remote).unwrap(),
        vec!["references/old.txt"]
    );
}

#[test]
fn check_with_no_receipts_is_empty() {
    let fixture = Fixture::new();
    let server = TestServer::github("unused", "unused");
    assert!(fixture
        .runtime()
        .block_on(fixture.service(&server).check_updates())
        .unwrap()
        .is_empty());
}

#[test]
fn incomplete_receipt_is_unavailable_without_network() {
    let fixture = Fixture::new();
    let id = fixture.install("incomplete", "old");
    Connection::open(&fixture.database_path)
        .unwrap()
        .execute(
            "UPDATE install_receipts SET commit_sha = NULL WHERE skill_id = ?1",
            [&id],
        )
        .unwrap();
    let server = TestServer::github("incomplete", "new");
    let item = fixture
        .runtime()
        .block_on(fixture.service(&server).check_updates())
        .unwrap()
        .remove(0);
    assert_eq!(item.status, SkillUpdateStatus::Unavailable);
}

#[test]
fn local_and_unknown_receipts_are_unavailable_without_network() {
    for source_type in ["local", "future-provider"] {
        let fixture = Fixture::new();
        let id = fixture.install("unsupported", "old");
        fixture.update_receipt_source(
            &id,
            source_type,
            "https://github.com/openai/codex",
            "skills/unsupported",
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);

        let item = fixture
            .runtime()
            .block_on(fixture.service_at(&origin).check_updates())
            .unwrap()
            .remove(0);
        assert_eq!(item.status, SkillUpdateStatus::Unavailable);
        assert!(item.reason.contains("安装凭据不完整"));
    }
}

#[test]
fn offline_github_source_degrades_only_its_receipt() {
    let fixture = Fixture::new();
    fixture.install("offline", "old");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    let item = fixture
        .runtime()
        .block_on(fixture.service_at(&origin).check_updates())
        .unwrap()
        .remove(0);
    assert_eq!(item.status, SkillUpdateStatus::Unavailable);
    assert_eq!(item.reason, "当前无法连接更新来源。");
}

#[test]
fn github_rate_limit_has_distinct_recovery_reason() {
    let fixture = Fixture::new();
    fixture.install("limited", "old");
    let server = TestServer::start(|_| response("429 Too Many Requests", b"private"));

    let item = fixture
        .runtime()
        .block_on(fixture.service(&server).check_updates())
        .unwrap()
        .remove(0);
    assert_eq!(item.status, SkillUpdateStatus::Unavailable);
    assert_eq!(item.reason, "GitHub 已限流，请稍后手动重试。");
}

#[test]
fn missing_skill_is_unavailable() {
    let fixture = Fixture::new();
    let id = fixture.install("missing", "old");
    fs::remove_dir_all(fixture.target("missing")).unwrap();
    let server = TestServer::github("missing", "new");
    let item = fixture
        .runtime()
        .block_on(fixture.service(&server).check_updates())
        .unwrap()
        .remove(0);
    assert_eq!(item.skill_id, id);
    assert_eq!(item.status, SkillUpdateStatus::Unavailable);
}

#[test]
fn local_modification_is_conflict_without_remote_overwrite() {
    let fixture = Fixture::new();
    fixture.install("conflict", "old");
    fs::write(
        fixture.target("conflict").join("SKILL.md"),
        skill("conflict", "local"),
    )
    .unwrap();
    let server = TestServer::github("conflict", "remote");
    let item = fixture
        .runtime()
        .block_on(fixture.service(&server).check_updates())
        .unwrap()
        .remove(0);
    assert_eq!(item.status, SkillUpdateStatus::Conflict);
    assert!(
        fs::read_to_string(fixture.target("conflict").join("SKILL.md"))
            .unwrap()
            .contains("local")
    );
}

#[test]
fn identical_remote_content_is_current_and_check_staging_is_removed() {
    let fixture = Fixture::new();
    fixture.install("current", "same");
    let server = TestServer::github("current", "same");
    let item = fixture
        .runtime()
        .block_on(fixture.service(&server).check_updates())
        .unwrap()
        .remove(0);
    assert_eq!(item.status, SkillUpdateStatus::Current);
    assert!(!fixture
        .app_local
        .join("update-staging")
        .read_dir()
        .unwrap()
        .next()
        .is_some());
}

#[test]
fn changed_remote_content_is_available_with_relative_summary() {
    let fixture = Fixture::new();
    fixture.install("available", "old");
    let server = TestServer::github("available", "new");
    let item = fixture
        .runtime()
        .block_on(fixture.service(&server).check_updates())
        .unwrap()
        .remove(0);
    assert_eq!(item.status, SkillUpdateStatus::Available);
    assert_eq!(item.available_commit.as_deref(), Some("22222222"));
    assert_eq!(item.changed_files, vec!["SKILL.md"]);
}

#[test]
fn market_update_uses_the_checked_commit_and_preserves_market_provenance() {
    let fixture = Fixture::new();
    let id = fixture.install("market-skill", "old");
    fixture.update_receipt_source(
        &id,
        "market",
        "https://github.com/openai/plugins",
        "plugins/alpha-plugin/skills/market-skill",
    );
    let server = TestServer::market("market-skill", "new");
    let service = fixture.service(&server);

    let item = fixture
        .runtime()
        .block_on(service.check_updates())
        .unwrap()
        .remove(0);
    assert_eq!(item.status, SkillUpdateStatus::Available);
    assert_eq!(item.source_type, "market");
    let plan = fixture
        .runtime()
        .block_on(service.plan_update(&id))
        .unwrap();
    service
        .execute_update(&plan.confirmation_token.unwrap().token)
        .unwrap();

    let receipt: (String, String) = Connection::open(&fixture.database_path)
        .unwrap()
        .query_row(
            "SELECT source_type, commit_sha FROM install_receipts WHERE skill_id = ?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(receipt, ("market".to_owned(), NEW_SHA.to_owned()));
    assert!(
        fs::read_to_string(fixture.target("market-skill").join("SKILL.md"))
            .unwrap()
            .contains("new")
    );
}

#[test]
fn same_name_in_repo_provider_is_not_selected_or_modified() {
    let fixture = Fixture::new();
    let repo = fixture.write_repo_skill("same-name", "repo");
    let id = fixture.install("same-name", "user-old");
    let server = TestServer::github("same-name", "user-new");
    let service = fixture.service(&server);

    let items = fixture.runtime().block_on(service.check_updates()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].skill_id, id);
    let plan = fixture
        .runtime()
        .block_on(service.plan_update(&id))
        .unwrap();
    service
        .execute_update(&plan.confirmation_token.unwrap().token)
        .unwrap();
    assert!(fs::read_to_string(repo.join("SKILL.md"))
        .unwrap()
        .contains("repo"));
    assert!(
        fs::read_to_string(fixture.target("same-name").join("SKILL.md"))
            .unwrap()
            .contains("user-new")
    );
}

#[test]
fn concurrent_manual_checks_use_isolated_staging_roots() {
    let fixture = Fixture::new();
    let id = fixture.install("concurrent", "old");
    let server = TestServer::github("concurrent", "new");
    let service = fixture.service(&server);
    let (first, second) = thread::scope(|scope| {
        let first = scope.spawn(|| fixture.runtime().block_on(service.check_updates()));
        let second = scope.spawn(|| fixture.runtime().block_on(service.check_updates()));
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_eq!(first.unwrap()[0].status, SkillUpdateStatus::Available);
    assert_eq!(second.unwrap()[0].status, SkillUpdateStatus::Available);
    assert!(fixture
        .app_local
        .join("update-staging")
        .read_dir()
        .unwrap()
        .next()
        .is_none());
    assert!(service.checked.lock().unwrap().contains_key(&id));
}

#[test]
fn cancelled_update_plan_removes_staging() {
    let fixture = Fixture::new();
    let id = fixture.install("cancel", "old");
    let server = TestServer::github("cancel", "new");
    let service = fixture.service(&server);
    fixture.runtime().block_on(service.check_updates()).unwrap();
    let plan = fixture
        .runtime()
        .block_on(service.plan_update(&id))
        .unwrap();
    let root = fixture.app_local.join("update-staging").join(&plan.plan.id);
    assert!(root.is_dir());
    fixture
        .operations
        .cancel_import(&plan.confirmation_token.unwrap().token)
        .unwrap();
    assert!(!root.exists());
}

#[test]
fn failed_plan_revalidation_removes_staging() {
    let fixture = Fixture::new();
    let id = fixture.install("plan-cleanup", "old");
    let server = TestServer::github("plan-cleanup", "new");
    let service = fixture.service(&server);
    fixture.runtime().block_on(service.check_updates()).unwrap();
    service
        .checked
        .lock()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .remote_hash = "changed-after-check".to_owned();

    assert_eq!(
        fixture
            .runtime()
            .block_on(service.plan_update(&id))
            .unwrap_err()
            .code,
        "update_receipt_changed"
    );
    assert!(fixture
        .app_local
        .join("update-staging")
        .read_dir()
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn successful_update_changes_files_and_receipt_together() {
    let fixture = Fixture::new();
    let id = fixture.install("success", "old");
    let server = TestServer::github("success", "new");
    let service = fixture.service(&server);
    fixture.runtime().block_on(service.check_updates()).unwrap();
    let plan = fixture
        .runtime()
        .block_on(service.plan_update(&id))
        .unwrap();
    service
        .execute_update(&plan.confirmation_token.unwrap().token)
        .unwrap();
    assert!(
        fs::read_to_string(fixture.target("success").join("SKILL.md"))
            .unwrap()
            .contains("new")
    );
    let receipt: (String, String) = Connection::open(&fixture.database_path)
        .unwrap()
        .query_row(
            "SELECT commit_sha, installed_hash FROM install_receipts WHERE skill_id = ?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(receipt.0, NEW_SHA);
    assert_eq!(
        receipt.1,
        fixture.catalog.current_content_hash(&id).unwrap()
    );
    assert_eq!(fixture.analysis_queue.enqueue_call_count(), 1);
}

#[test]
fn database_failure_restores_previous_files() {
    let fixture = Fixture::new();
    let id = fixture.install("db-rollback", "old");
    let server = TestServer::github("db-rollback", "new");
    let service = fixture.service(&server);
    fixture.runtime().block_on(service.check_updates()).unwrap();
    let plan = fixture
        .runtime()
        .block_on(service.plan_update(&id))
        .unwrap();
    fixture.operations.force_update_database_failure();
    assert_eq!(
        service
            .execute_update(&plan.confirmation_token.unwrap().token)
            .unwrap_err()
            .code,
        "update_failed"
    );
    assert!(
        fs::read_to_string(fixture.target("db-rollback").join("SKILL.md"))
            .unwrap()
            .contains("old")
    );
    assert_eq!(fixture.analysis_queue.enqueue_call_count(), 0);
}

#[test]
fn catalog_failure_restores_files_and_old_receipt() {
    let fixture = Fixture::new();
    let id = fixture.install("catalog-rollback", "old");
    let server = TestServer::github("catalog-rollback", "new");
    let service = fixture.service(&server);
    fixture.runtime().block_on(service.check_updates()).unwrap();
    let plan = fixture
        .runtime()
        .block_on(service.plan_update(&id))
        .unwrap();
    fixture.operations.force_update_catalog_failure();
    assert_eq!(
        service
            .execute_update(&plan.confirmation_token.unwrap().token)
            .unwrap_err()
            .code,
        "update_failed"
    );
    let commit: String = Connection::open(&fixture.database_path)
        .unwrap()
        .query_row(
            "SELECT commit_sha FROM install_receipts WHERE skill_id = ?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(commit, OLD_SHA);
    assert!(
        fs::read_to_string(fixture.target("catalog-rollback").join("SKILL.md"))
            .unwrap()
            .contains("old")
    );
}

#[test]
fn failed_rollback_returns_partial_and_preserves_the_backup() {
    let fixture = Fixture::new();
    let id = fixture.install("partial", "old");
    let server = TestServer::github("partial", "new");
    let service = fixture.service(&server);
    fixture.runtime().block_on(service.check_updates()).unwrap();
    let plan = fixture
        .runtime()
        .block_on(service.plan_update(&id))
        .unwrap();
    let backup = fixture
        .app_local
        .join("update-backups")
        .join(&plan.plan.id)
        .join("partial/SKILL.md");
    fixture.operations.force_update_database_failure();
    fixture.operations.force_update_rollback_failure();

    assert_eq!(
        service
            .execute_update(&plan.confirmation_token.unwrap().token)
            .unwrap_err()
            .code,
        "update_partial"
    );
    assert!(fs::read_to_string(backup).unwrap().contains("old"));
}

#[test]
fn expired_update_confirmation_removes_staging() {
    let fixture = Fixture::new();
    let id = fixture.install("expired", "old");
    let server = TestServer::github("expired", "new");
    let service = fixture.service(&server);
    fixture.runtime().block_on(service.check_updates()).unwrap();
    let plan = fixture
        .runtime()
        .block_on(service.plan_update(&id))
        .unwrap();
    let root = fixture.app_local.join("update-staging").join(&plan.plan.id);
    let token = plan.confirmation_token.unwrap().token;
    fixture.operations.expire_confirmation(&token);
    assert_eq!(
        service.execute_update(&token).unwrap_err().code,
        "confirmation_token_expired"
    );
    assert!(!root.exists());
}
