use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use rusqlite::Connection;
use serde_json::json;
use tempfile::TempDir;

use crate::{
    catalog::SkillCatalog, db, observability::DiagnosticService, operations::OperationsService,
    providers::ProviderRoots,
};

use super::*;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
static NETWORK_TEST_LOCK: Mutex<()> = Mutex::new(());

fn network_test_guard() -> std::sync::MutexGuard<'static, ()> {
    NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn plugin(
    name: &str,
    source: &str,
    installation: &str,
    products: Option<Vec<&str>>,
) -> serde_json::Value {
    let mut policy = json!({ "installation": installation });
    if let Some(products) = products {
        policy["products"] = json!(products);
    }
    json!({
        "name": name,
        "source": source,
        "category": "Developer Tools",
        "description": "A fixture plugin.",
        "policy": policy,
        "future_field": { "ignored": true }
    })
}

fn document(plugins: Vec<serde_json::Value>) -> MarketplaceDocument {
    MarketplaceDocument {
        name: OFFICIAL_PROVIDER_NAME.to_owned(),
        plugins,
    }
}

fn tree(entries: Vec<TreeEntry>) -> TreeResponse {
    TreeResponse {
        sha: SHA.to_owned(),
        truncated: false,
        tree: entries,
    }
}

fn blob(path: &str, size: u64) -> TreeEntry {
    TreeEntry {
        path: path.to_owned(),
        mode: "100644".to_owned(),
        kind: "blob".to_owned(),
        size: Some(size),
    }
}

fn valid_snapshot() -> CachedMarketSnapshot {
    build_snapshot(
        document(vec![plugin(
            "alpha-plugin",
            "./plugins/alpha-plugin",
            "AVAILABLE",
            Some(vec!["CODEX"]),
        )]),
        tree(vec![blob(
            "plugins/alpha-plugin/skills/alpha-skill/SKILL.md",
            80,
        )]),
        SHA.to_owned(),
        42,
    )
    .unwrap()
}

#[test]
fn unknown_marketplace_fields_are_ignored() {
    let snapshot = valid_snapshot();
    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.items[0].skill_name, "alpha-skill");
}

#[test]
fn malformed_plugin_is_isolated_from_valid_items() {
    let snapshot = build_snapshot(
        document(vec![
            json!({ "name": "broken" }),
            plugin("alpha-plugin", "./plugins/alpha-plugin", "AVAILABLE", None),
        ]),
        tree(vec![blob(
            "plugins/alpha-plugin/skills/alpha-skill/SKILL.md",
            80,
        )]),
        SHA.to_owned(),
        42,
    )
    .unwrap();
    assert_eq!(snapshot.items.len(), 1);
}

#[test]
fn marketplace_source_traversal_is_rejected_per_item() {
    let unsafe_plugin = serde_json::from_value::<MarketplacePlugin>(plugin(
        "alpha-plugin",
        "./plugins/../alpha-plugin",
        "AVAILABLE",
        None,
    ))
    .unwrap();
    assert!(validate_plugin(&unsafe_plugin).is_none());
    let snapshot = build_snapshot(
        document(vec![plugin(
            "alpha-plugin",
            "./plugins/../alpha-plugin",
            "AVAILABLE",
            None,
        )]),
        tree(vec![blob(
            "plugins/alpha-plugin/skills/alpha-skill/SKILL.md",
            80,
        )]),
        SHA.to_owned(),
        42,
    )
    .unwrap();
    assert!(snapshot.items.is_empty());
}

#[test]
fn absolute_or_parent_tree_paths_reject_the_snapshot() {
    for path in [
        "/plugins/alpha-plugin/skills/alpha-skill/SKILL.md",
        "plugins/../alpha-plugin/skills/alpha-skill/SKILL.md",
    ] {
        let result = build_snapshot(
            document(vec![]),
            tree(vec![blob(path, 80)]),
            SHA.to_owned(),
            42,
        );
        assert_eq!(result.unwrap_err(), MarketFailure::InvalidIndex);
    }
}

#[test]
fn symlink_inside_skill_excludes_that_market_item() {
    let mut symlink = blob("plugins/alpha-plugin/skills/alpha-skill/references/link", 4);
    symlink.mode = "120000".to_owned();
    let snapshot = build_snapshot(
        document(vec![plugin(
            "alpha-plugin",
            "./plugins/alpha-plugin",
            "AVAILABLE",
            None,
        )]),
        tree(vec![
            blob("plugins/alpha-plugin/skills/alpha-skill/SKILL.md", 80),
            symlink,
        ]),
        SHA.to_owned(),
        42,
    )
    .unwrap();
    assert!(snapshot.items.is_empty());
}

#[test]
fn only_available_codex_products_are_admitted() {
    let snapshot = build_snapshot(
        document(vec![
            plugin(
                "alpha-plugin",
                "./plugins/alpha-plugin",
                "UNAVAILABLE",
                Some(vec!["CODEX"]),
            ),
            plugin(
                "beta-plugin",
                "./plugins/beta-plugin",
                "AVAILABLE",
                Some(vec!["CHATGPT"]),
            ),
            plugin("gamma-plugin", "./plugins/gamma-plugin", "AVAILABLE", None),
        ]),
        tree(vec![
            blob("plugins/alpha-plugin/skills/alpha/SKILL.md", 1),
            blob("plugins/beta-plugin/skills/beta/SKILL.md", 1),
            blob("plugins/gamma-plugin/skills/gamma/SKILL.md", 1),
        ]),
        SHA.to_owned(),
        42,
    )
    .unwrap();
    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.items[0].plugin_name, "gamma-plugin");
}

#[test]
fn duplicate_normalized_market_ids_are_not_returned_twice() {
    let snapshot = build_snapshot(
        document(vec![
            plugin("alpha-plugin", "./plugins/alpha-plugin", "AVAILABLE", None),
            plugin("alpha-plugin", "./plugins/alpha-plugin", "AVAILABLE", None),
        ]),
        tree(vec![blob(
            "plugins/alpha-plugin/skills/alpha-skill/SKILL.md",
            80,
        )]),
        SHA.to_owned(),
        42,
    )
    .unwrap();
    assert_eq!(snapshot.items.len(), 1);
}

#[test]
fn truncated_or_wrong_sha_tree_is_rejected() {
    let mut truncated = tree(vec![]);
    truncated.truncated = true;
    assert_eq!(
        build_snapshot(document(vec![]), truncated, SHA.to_owned(), 42).unwrap_err(),
        MarketFailure::InvalidIndex
    );
    let mut wrong_sha = tree(vec![]);
    wrong_sha.sha = "ffffffffffffffffffffffffffffffffffffffff".to_owned();
    assert_eq!(
        build_snapshot(document(vec![]), wrong_sha, SHA.to_owned(), 42).unwrap_err(),
        MarketFailure::InvalidIndex
    );
}

#[test]
fn cache_round_trip_preserves_fixed_commit_and_internal_path() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("market-cache.json");
    let snapshot = valid_snapshot();
    write_cache_atomic(&path, &snapshot).unwrap();
    assert_eq!(load_cache(&path), Some(snapshot));
}

#[test]
fn failed_cache_validation_keeps_the_previous_snapshot() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("market-cache.json");
    let snapshot = valid_snapshot();
    write_cache_atomic(&path, &snapshot).unwrap();
    let original = fs::read(&path).unwrap();
    let mut invalid = snapshot;
    invalid.commit_sha = "main".to_owned();
    assert_eq!(
        write_cache_atomic(&path, &invalid).unwrap_err(),
        MarketFailure::InvalidIndex
    );
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn symlink_cache_is_never_loaded_or_replaced() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("target.json");
    let path = temporary.path().join("market-cache.json");
    fs::write(&target, b"preserve").unwrap();
    symlink(&target, &path).unwrap();
    assert!(load_cache(&path).is_none());
    assert_eq!(
        write_cache_atomic(&path, &valid_snapshot()).unwrap_err(),
        MarketFailure::StorageUnavailable
    );
    assert_eq!(fs::read(&target).unwrap(), b"preserve");
}

#[test]
fn selected_tree_rejects_missing_manifest_symlink_and_unsupported_mode() {
    let directory = "plugins/alpha-plugin/skills/alpha-skill";
    assert_eq!(
        validate_selected_tree(&[], directory).unwrap_err().code,
        "market_item_unavailable"
    );
    let mut symlink = blob(&format!("{directory}/SKILL.md"), 1);
    symlink.mode = "120000".to_owned();
    assert_eq!(
        validate_selected_tree(&[symlink], directory)
            .unwrap_err()
            .code,
        "market_source_invalid"
    );
    let mut submodule = blob(&format!("{directory}/SKILL.md"), 1);
    submodule.mode = "160000".to_owned();
    assert_eq!(
        validate_selected_tree(&[submodule], directory)
            .unwrap_err()
            .code,
        "market_source_invalid"
    );
}

#[test]
fn selected_files_enforce_count_single_file_and_total_limits() {
    let prefix = "plugins/alpha-plugin/skills/alpha-skill/";
    let too_many = (0..=crate::operations::MAX_IMPORT_FILES)
        .map(|index| blob(&format!("{prefix}references/{index}.txt"), 1))
        .collect::<Vec<_>>();
    assert_eq!(
        validate_market_file_limits(&too_many, prefix)
            .unwrap_err()
            .code,
        "market_source_invalid"
    );
    assert_eq!(
        validate_market_file_limits(
            &[blob(
                &format!("{prefix}SKILL.md"),
                crate::operations::MAX_IMPORT_TEXT_BYTES + 1,
            )],
            prefix,
        )
        .unwrap_err()
        .code,
        "market_source_invalid"
    );
    assert_eq!(
        validate_market_file_limits(
            &[
                blob(
                    &format!("{prefix}assets/one.bin"),
                    crate::operations::MAX_IMPORT_RESOURCE_BYTES,
                ),
                blob(
                    &format!("{prefix}assets/two.bin"),
                    crate::operations::MAX_IMPORT_RESOURCE_BYTES,
                ),
                blob(&format!("{prefix}assets/three.bin"), 1),
            ],
            prefix,
        )
        .unwrap_err()
        .code,
        "market_source_invalid"
    );
}

#[test]
fn tree_and_market_item_counts_are_bounded() {
    let oversized_tree = (0..=TREE_ENTRY_LIMIT)
        .map(|index| blob(&format!("safe/{index}"), 1))
        .collect::<Vec<_>>();
    assert_eq!(
        build_snapshot(document(vec![]), tree(oversized_tree), SHA.to_owned(), 42,).unwrap_err(),
        MarketFailure::InvalidIndex
    );

    let plugins = (0..=MARKET_ITEM_LIMIT)
        .map(|index| {
            plugin(
                &format!("plugin-{index}"),
                &format!("./plugins/plugin-{index}"),
                "AVAILABLE",
                None,
            )
        })
        .collect::<Vec<_>>();
    let entries = (0..=MARKET_ITEM_LIMIT)
        .map(|index| blob(&format!("plugins/plugin-{index}/skills/skill/SKILL.md"), 1))
        .collect::<Vec<_>>();
    assert_eq!(
        build_snapshot(document(plugins), tree(entries), SHA.to_owned(), 42).unwrap_err(),
        MarketFailure::InvalidIndex
    );
}

#[test]
fn declared_http_response_size_is_rejected_before_body_read() {
    let _network_guard = network_test_guard();
    let server = TestServer::start(|_| {
        b"HTTP/1.1 200 OK\r\nContent-Length: 1025\r\nConnection: close\r\n\r\n".to_vec()
    });
    let endpoints = MarketEndpoints::loopback(&server.origin);
    let client = market_client().unwrap();
    let url = endpoint_url(&endpoints.api_base, &["oversized"]).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let response = runtime.block_on(request(&client, url, &endpoints)).unwrap();
    assert_eq!(
        runtime.block_on(read_limited(response, 1024)).unwrap_err(),
        MarketFailure::ResponseTooLarge
    );
}

struct ServiceFixture {
    temporary: TempDir,
    home: PathBuf,
    database_path: PathBuf,
    operations: Arc<OperationsService>,
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
        let operations = Arc::new(OperationsService::new(
            Some(database_path.clone()),
            Some(temporary.path().join("app-local")),
            SkillCatalog::with_index_path(roots, database_path.clone()),
            DiagnosticService::new(None, None),
        ));
        Self {
            temporary,
            home,
            database_path,
            operations,
        }
    }
}

#[test]
fn no_cache_returns_explicit_unavailable_state() {
    let fixture = ServiceFixture::new();
    let service = MarketService::new(
        Some(fixture.temporary.path().join("market-cache.json")),
        fixture.operations,
    );
    let catalog = service.catalog();
    assert_eq!(catalog.status, MarketStatus::Unavailable);
    assert_eq!(catalog.issue.unwrap().code, "market_cache_missing");
}

#[test]
fn refresh_uses_one_commit_for_marketplace_and_tree() {
    let _network_guard = network_test_guard();
    let fixture = ServiceFixture::new();
    let skill = valid_skill("alpha-skill");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let server = TestServer::start(move |path| {
        captured.lock().unwrap().push(path.to_owned());
        market_response(path, &skill)
    });
    let service = MarketService::with_endpoints(
        Some(fixture.temporary.path().join("market-cache.json")),
        fixture.operations,
        MarketEndpoints::loopback(&server.origin),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let catalog = runtime.block_on(service.refresh());
    assert_eq!(catalog.status, MarketStatus::Ready);
    assert_eq!(catalog.commit_sha.as_deref(), Some(SHA));
    let requests = requests.lock().unwrap();
    assert!(requests
        .iter()
        .any(|path| path == &format!("/openai/plugins/{SHA}/.agents/plugins/marketplace.json")));
    assert!(requests
        .iter()
        .any(|path| { path == &format!("/repos/openai/plugins/git/trees/{SHA}?recursive=1") }));
}

#[test]
fn failed_refresh_returns_stale_cache_without_overwriting_it() {
    let _network_guard = network_test_guard();
    let fixture = ServiceFixture::new();
    let cache_path = fixture.temporary.path().join("market-cache.json");
    let snapshot = valid_snapshot();
    write_cache_atomic(&cache_path, &snapshot).unwrap();
    let server = TestServer::start(|_| response("500 Internal Server Error", b"failed"));
    let service = MarketService::with_endpoints(
        Some(cache_path.clone()),
        fixture.operations,
        MarketEndpoints::loopback(&server.origin),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let catalog = runtime.block_on(service.refresh());
    assert_eq!(catalog.status, MarketStatus::Stale);
    assert_eq!(catalog.items.len(), 1);
    assert_eq!(load_cache(&cache_path), Some(snapshot));
}

#[test]
fn market_plan_executes_through_shared_import_and_persists_receipt() {
    let _network_guard = network_test_guard();
    let fixture = ServiceFixture::new();
    let skill = valid_skill("alpha-skill");
    let server = TestServer::start(move |path| market_response(path, &skill));
    let service = MarketService::with_endpoints(
        Some(fixture.temporary.path().join("market-cache.json")),
        Arc::clone(&fixture.operations),
        MarketEndpoints::loopback(&server.origin),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let catalog = runtime.block_on(service.refresh());
    let plan = runtime
        .block_on(service.plan_import(&catalog.items[0].id))
        .unwrap();
    let result = fixture
        .operations
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap();
    assert!(fixture
        .home
        .join(".agents/skills/alpha-skill/SKILL.md")
        .is_file());
    let connection = Connection::open(&fixture.database_path).unwrap();
    let receipt = connection
        .query_row(
            "SELECT source_type, source_url, repo_ref, commit_sha, subdirectory, installed_hash FROM install_receipts WHERE skill_id = ?1",
            [&result.skill_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(receipt.0, "market");
    assert_eq!(receipt.1, OFFICIAL_REPOSITORY_URL);
    assert_eq!(receipt.2, "main");
    assert_eq!(receipt.3, SHA);
    assert_eq!(receipt.4, "plugins/alpha-plugin/skills/alpha-skill");
    assert_eq!(receipt.5, result.installed_hash);
    let refreshed = service.catalog();
    assert!(refreshed.items[0].installed);
}

#[test]
fn same_name_local_skill_is_not_marked_as_market_installed() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let target = home.join(".agents/skills/alpha-skill");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), valid_skill("alpha-skill")).unwrap();
    let database_path = temporary.path().join("data.db");
    assert!(matches!(
        db::initialize(database_path.clone()),
        db::AppDatabase::Ready(_)
    ));
    let roots = ProviderRoots::new(
        home,
        temporary.path().join("repository"),
        temporary.path().join("plugin-cache"),
    );
    let catalog = SkillCatalog::with_index_path(roots, database_path.clone());
    catalog.scan_skills();
    let operations = Arc::new(OperationsService::new(
        Some(database_path),
        Some(temporary.path().join("app-local")),
        catalog,
        DiagnosticService::new(None, None),
    ));
    let cache_path = temporary.path().join("market-cache.json");
    write_cache_atomic(&cache_path, &valid_snapshot()).unwrap();
    let service = MarketService::new(Some(cache_path), operations);
    assert!(!service.catalog().items[0].installed);
}

#[test]
fn cancelling_market_plan_removes_bound_staging() {
    let _network_guard = network_test_guard();
    let fixture = ServiceFixture::new();
    let skill = valid_skill("alpha-skill");
    let server = TestServer::start(move |path| market_response(path, &skill));
    let service = MarketService::with_endpoints(
        Some(fixture.temporary.path().join("market-cache.json")),
        Arc::clone(&fixture.operations),
        MarketEndpoints::loopback(&server.origin),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let catalog = runtime.block_on(service.refresh());
    let plan = runtime
        .block_on(service.plan_import(&catalog.items[0].id))
        .unwrap();
    let staging = fixture.temporary.path().join("app-local/github-staging");
    assert_eq!(fs::read_dir(&staging).unwrap().count(), 1);
    fixture
        .operations
        .cancel_import(&plan.confirmation_token.unwrap().token)
        .unwrap();
    assert_eq!(fs::read_dir(&staging).unwrap().count(), 0);
}

#[test]
fn market_conflict_has_no_token_and_leaves_no_staging() {
    let _network_guard = network_test_guard();
    let fixture = ServiceFixture::new();
    let target = fixture.home.join(".agents/skills/alpha-skill");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), valid_skill("alpha-skill")).unwrap();
    let skill = valid_skill("alpha-skill");
    let server = TestServer::start(move |path| market_response(path, &skill));
    let service = MarketService::with_endpoints(
        Some(fixture.temporary.path().join("market-cache.json")),
        Arc::clone(&fixture.operations),
        MarketEndpoints::loopback(&server.origin),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let catalog = runtime.block_on(service.refresh());
    let plan = runtime
        .block_on(service.plan_import(&catalog.items[0].id))
        .unwrap();
    assert_eq!(
        plan.plan.status,
        crate::operations::OperationPlanStatus::Conflict
    );
    assert!(plan.confirmation_token.is_none());
    let staging = fixture.temporary.path().join("app-local/github-staging");
    assert_eq!(fs::read_dir(staging).unwrap().count(), 0);
}

#[test]
fn modified_market_staging_is_rejected_and_cleaned_before_write() {
    let _network_guard = network_test_guard();
    let fixture = ServiceFixture::new();
    let skill = valid_skill("alpha-skill");
    let server = TestServer::start(move |path| market_response(path, &skill));
    let service = MarketService::with_endpoints(
        Some(fixture.temporary.path().join("market-cache.json")),
        Arc::clone(&fixture.operations),
        MarketEndpoints::loopback(&server.origin),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let catalog = runtime.block_on(service.refresh());
    let plan = runtime
        .block_on(service.plan_import(&catalog.items[0].id))
        .unwrap();
    let staging_root = fixture.temporary.path().join("app-local/github-staging");
    let operation_root = fs::read_dir(&staging_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(
        operation_root.join("home/.agents/skills/alpha-skill/SKILL.md"),
        valid_skill("changed-skill"),
    )
    .unwrap();
    let error = fixture
        .operations
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap_err();
    assert_eq!(error.code, "source_changed");
    assert!(!fixture.home.join(".agents/skills/alpha-skill").exists());
    assert_eq!(fs::read_dir(staging_root).unwrap().count(), 0);
}

fn valid_skill(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: A safe market fixture.\n---\n# Overview\nFixture body.\n"
    )
}

fn market_response(path: &str, skill: &str) -> Vec<u8> {
    if path == "/repos/openai/plugins/commits/main" {
        return json_response(&json!({ "sha": SHA }));
    }
    if path == format!("/openai/plugins/{SHA}/.agents/plugins/marketplace.json") {
        return json_response(&json!({
            "name": OFFICIAL_PROVIDER_NAME,
            "plugins": [plugin(
                "alpha-plugin",
                "./plugins/alpha-plugin",
                "AVAILABLE",
                Some(vec!["CODEX"]),
            )]
        }));
    }
    if path == format!("/repos/openai/plugins/git/trees/{SHA}?recursive=1") {
        return json_response(&json!({
            "sha": SHA,
            "truncated": false,
            "tree": [{
                "path": "plugins/alpha-plugin/skills/alpha-skill/SKILL.md",
                "mode": "100644",
                "type": "blob",
                "size": skill.len()
            }]
        }));
    }
    if path == format!("/openai/plugins/{SHA}/plugins/alpha-plugin/skills/alpha-skill/SKILL.md") {
        return response("200 OK", skill.as_bytes());
    }
    response("404 Not Found", b"missing")
}

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
    // Accepted sockets can inherit the listener's nonblocking mode on macOS.
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
    let request_text = String::from_utf8_lossy(&request);
    let path = request_text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let _ = stream.write_all(&handler(path));
    let _ = stream.flush();
}

fn json_response(value: &serde_json::Value) -> Vec<u8> {
    response("200 OK", serde_json::to_vec(value).unwrap().as_slice())
}

fn response(status: &str, body: &[u8]) -> Vec<u8> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    header
        .into_bytes()
        .into_iter()
        .chain(body.iter().copied())
        .collect()
}
