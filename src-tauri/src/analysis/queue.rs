use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Semaphore;

use crate::observability::{
    DiagnosticDomain, DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel, DiagnosticRecord,
    DiagnosticRecoveryCode, DiagnosticResult,
};

use super::{AnalysisResult, AnalysisRunStatus, AnalysisService, AnalysisServiceError};

const DEFAULT_CONCURRENCY: usize = 2;
const MAX_CONCURRENCY: usize = 4;
const ANALYSIS_PROGRESS_EVENT: &str = "analysis_progress";

#[async_trait]
trait AnalysisExecutor: Send + Sync {
    fn job_key(&self, skill_id: &str) -> Result<Option<String>, AnalysisServiceError>;
    async fn execute(
        &self,
        skill_id: &str,
        force: bool,
    ) -> Result<AnalysisResult, AnalysisServiceError>;
}

#[async_trait]
impl AnalysisExecutor for AnalysisService {
    fn job_key(&self, skill_id: &str) -> Result<Option<String>, AnalysisServiceError> {
        AnalysisService::job_key(self, skill_id)
    }

    async fn execute(
        &self,
        skill_id: &str,
        force: bool,
    ) -> Result<AnalysisResult, AnalysisServiceError> {
        self.analyze(skill_id, force).await
    }
}

pub trait AnalysisProgressSink: Send + Sync {
    fn emit(&self, progress: &AnalysisProgress);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopAnalysisProgressSink;

impl AnalysisProgressSink for NoopAnalysisProgressSink {
    fn emit(&self, _progress: &AnalysisProgress) {}
}

#[derive(Clone)]
pub struct TauriAnalysisProgressSink {
    app: AppHandle,
    last_statuses: Arc<Mutex<HashMap<String, AnalysisJobStatus>>>,
}

impl TauriAnalysisProgressSink {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            last_statuses: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl AnalysisProgressSink for TauriAnalysisProgressSink {
    fn emit(&self, progress: &AnalysisProgress) {
        let _ = self.app.emit(ANALYSIS_PROGRESS_EVENT, progress);
        let changed = {
            let mut last_statuses = self
                .last_statuses
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let changed = progress
                .jobs
                .iter()
                .filter(|job| {
                    last_statuses
                        .get(&job.job_id)
                        .is_none_or(|status| *status != job.status)
                })
                .cloned()
                .collect::<Vec<_>>();
            for job in &changed {
                last_statuses.insert(job.job_id.clone(), job.status);
            }
            changed
        };
        for job in changed {
            if let Some(record) = analysis_job_record(&job) {
                crate::diagnostics::emit(record);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisJobStatus {
    Queued,
    Running,
    Ready,
    Stale,
    Failed,
    Degraded,
    NotConfigured,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AnalysisJobView {
    pub job_id: String,
    pub skill_id: String,
    pub analysis_key: Option<String>,
    pub status: AnalysisJobStatus,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AnalysisProgress {
    pub total: usize,
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub degraded: usize,
    pub jobs: Vec<AnalysisJobView>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AnalysisEnqueueResult {
    pub job_id: Option<String>,
    pub status: AnalysisJobStatus,
    pub deduplicated: bool,
}

#[derive(Clone)]
pub struct AnalysisQueue {
    inner: Arc<QueueInner>,
}

struct QueueInner {
    executor: Arc<dyn AnalysisExecutor>,
    sink: Arc<dyn AnalysisProgressSink>,
    semaphore: Arc<Semaphore>,
    jobs: Mutex<HashMap<String, AnalysisJobView>>,
    #[cfg(test)]
    enqueue_calls: AtomicUsize,
}

impl AnalysisQueue {
    pub fn new(service: Arc<AnalysisService>, sink: Arc<dyn AnalysisProgressSink>) -> Self {
        Self::with_executor(service, DEFAULT_CONCURRENCY, sink)
    }

    fn with_executor(
        executor: Arc<dyn AnalysisExecutor>,
        concurrency: usize,
        sink: Arc<dyn AnalysisProgressSink>,
    ) -> Self {
        Self {
            inner: Arc::new(QueueInner {
                executor,
                sink,
                semaphore: Arc::new(Semaphore::new(concurrency.clamp(1, MAX_CONCURRENCY))),
                jobs: Mutex::new(HashMap::new()),
                #[cfg(test)]
                enqueue_calls: AtomicUsize::new(0),
            }),
        }
    }

    pub fn enqueue(
        &self,
        skill_id: String,
        force: bool,
    ) -> Result<AnalysisEnqueueResult, AnalysisServiceError> {
        #[cfg(test)]
        self.inner.enqueue_calls.fetch_add(1, Ordering::SeqCst);
        let Some(analysis_key) = self.inner.executor.job_key(&skill_id)? else {
            return Ok(AnalysisEnqueueResult {
                job_id: None,
                status: AnalysisJobStatus::NotConfigured,
                deduplicated: false,
            });
        };
        let job_id = format!("analysis-job:{analysis_key}");
        {
            let mut jobs = self
                .inner
                .jobs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if jobs.get(&analysis_key).is_some_and(|job| {
                matches!(
                    job.status,
                    AnalysisJobStatus::Queued | AnalysisJobStatus::Running
                )
            }) {
                return Ok(AnalysisEnqueueResult {
                    job_id: Some(job_id),
                    status: jobs[&analysis_key].status,
                    deduplicated: true,
                });
            }
            jobs.insert(
                analysis_key.clone(),
                AnalysisJobView {
                    job_id: job_id.clone(),
                    skill_id: skill_id.clone(),
                    analysis_key: Some(analysis_key.clone()),
                    status: AnalysisJobStatus::Queued,
                },
            );
        }
        self.emit_progress();
        let queue = self.clone();
        tokio::spawn(async move {
            queue.run_job(analysis_key, skill_id, force).await;
        });
        Ok(AnalysisEnqueueResult {
            job_id: Some(job_id),
            status: AnalysisJobStatus::Queued,
            deduplicated: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn enqueue_call_count(&self) -> usize {
        self.inner.enqueue_calls.load(Ordering::SeqCst)
    }

    pub fn enqueue_many(
        &self,
        skill_ids: Vec<String>,
        force: bool,
    ) -> Vec<Result<AnalysisEnqueueResult, AnalysisServiceError>> {
        skill_ids
            .into_iter()
            .map(|skill_id| self.enqueue(skill_id, force))
            .collect()
    }

    pub fn progress(&self) -> AnalysisProgress {
        let jobs = self
            .inner
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        progress_from_jobs(jobs)
    }

    async fn run_job(&self, analysis_key: String, skill_id: String, force: bool) {
        let Ok(_permit) = Arc::clone(&self.inner.semaphore).acquire_owned().await else {
            self.set_status(&analysis_key, AnalysisJobStatus::Failed);
            return;
        };
        self.set_status(&analysis_key, AnalysisJobStatus::Running);
        let status = match self.inner.executor.execute(&skill_id, force).await {
            Ok(result) => job_status(result.status),
            Err(_) => AnalysisJobStatus::Failed,
        };
        self.set_status(&analysis_key, status);
    }

    fn set_status(&self, analysis_key: &str, status: AnalysisJobStatus) {
        if let Some(job) = self
            .inner
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_mut(analysis_key)
        {
            job.status = status;
        }
        self.emit_progress();
    }

    fn emit_progress(&self) {
        self.inner.sink.emit(&self.progress());
    }
}

#[tauri::command]
pub async fn analyze_skill(
    queue: State<'_, AnalysisQueue>,
    skill_id: String,
    force: Option<bool>,
) -> Result<AnalysisEnqueueResult, AnalysisServiceError> {
    queue.enqueue(skill_id, force.unwrap_or(false))
}

fn job_status(status: AnalysisRunStatus) -> AnalysisJobStatus {
    match status {
        AnalysisRunStatus::NotRequested => AnalysisJobStatus::Queued,
        AnalysisRunStatus::NotConfigured => AnalysisJobStatus::NotConfigured,
        AnalysisRunStatus::Ready => AnalysisJobStatus::Ready,
        AnalysisRunStatus::Stale => AnalysisJobStatus::Stale,
        AnalysisRunStatus::Failed => AnalysisJobStatus::Failed,
        AnalysisRunStatus::Degraded => AnalysisJobStatus::Degraded,
    }
}

fn analysis_job_record(job: &AnalysisJobView) -> Option<DiagnosticRecord> {
    let record = match job.status {
        AnalysisJobStatus::Queued => DiagnosticRecord::new(
            DiagnosticLevel::Info,
            DiagnosticDomain::Analysis,
            DiagnosticEventCode::AnalysisQueued,
            DiagnosticResult::Started,
        ),
        AnalysisJobStatus::Ready | AnalysisJobStatus::Stale => DiagnosticRecord::new(
            DiagnosticLevel::Info,
            DiagnosticDomain::Analysis,
            DiagnosticEventCode::AnalysisCompleted,
            DiagnosticResult::Succeeded,
        ),
        AnalysisJobStatus::Degraded => DiagnosticRecord::new(
            DiagnosticLevel::Warning,
            DiagnosticDomain::Analysis,
            DiagnosticEventCode::AnalysisCompleted,
            DiagnosticResult::Degraded,
        ),
        AnalysisJobStatus::Failed => DiagnosticRecord::new(
            DiagnosticLevel::Error,
            DiagnosticDomain::Analysis,
            DiagnosticEventCode::AnalysisFailed,
            DiagnosticResult::Failed,
        )
        .with_error(
            DiagnosticErrorCode::AnalysisFailed,
            true,
            DiagnosticRecoveryCode::Retry,
        ),
        AnalysisJobStatus::NotConfigured => DiagnosticRecord::new(
            DiagnosticLevel::Warning,
            DiagnosticDomain::Analysis,
            DiagnosticEventCode::AnalysisFailed,
            DiagnosticResult::Failed,
        )
        .with_error(
            DiagnosticErrorCode::AnalysisNotConfigured,
            false,
            DiagnosticRecoveryCode::CheckSettings,
        ),
        AnalysisJobStatus::Running => return None,
    };
    Some(record.with_entity_ref(&job.skill_id))
}

fn progress_from_jobs(mut jobs: Vec<AnalysisJobView>) -> AnalysisProgress {
    jobs.sort_by(|left, right| left.job_id.cmp(&right.job_id));
    AnalysisProgress {
        total: jobs.len(),
        queued: jobs
            .iter()
            .filter(|job| job.status == AnalysisJobStatus::Queued)
            .count(),
        running: jobs
            .iter()
            .filter(|job| job.status == AnalysisJobStatus::Running)
            .count(),
        completed: jobs
            .iter()
            .filter(|job| {
                matches!(
                    job.status,
                    AnalysisJobStatus::Ready
                        | AnalysisJobStatus::Stale
                        | AnalysisJobStatus::NotConfigured
                )
            })
            .count(),
        failed: jobs
            .iter()
            .filter(|job| job.status == AnalysisJobStatus::Failed)
            .count(),
        degraded: jobs
            .iter()
            .filter(|job| job.status == AnalysisJobStatus::Degraded)
            .count(),
        jobs,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::{Duration, Instant},
    };

    use async_trait::async_trait;

    use crate::analysis::{
        queue::{AnalysisExecutor, AnalysisProgressSink},
        AnalysisEnqueueResult, AnalysisJobStatus, AnalysisProgress, AnalysisQueue, AnalysisResult,
        AnalysisRunStatus, AnalysisServiceError, AnalysisServiceErrorCode, RedactionCounts,
    };

    struct FixtureExecutor {
        keys: HashMap<String, Option<String>>,
        running: AtomicUsize,
        maximum: AtomicUsize,
        calls: AtomicUsize,
        delay: Duration,
        failed_skill: Option<String>,
    }

    impl FixtureExecutor {
        fn new(skill_count: usize, delay: Duration) -> Self {
            Self {
                keys: (0..skill_count)
                    .map(|index| (format!("skill-{index}"), Some(format!("key-{index}"))))
                    .collect(),
                running: AtomicUsize::new(0),
                maximum: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
                delay,
                failed_skill: None,
            }
        }

        fn with_duplicate_key(mut self) -> Self {
            self.keys
                .insert("skill-1".to_owned(), Some("key-0".to_owned()));
            self
        }

        fn with_failed_skill(mut self, skill_id: &str) -> Self {
            self.failed_skill = Some(skill_id.to_owned());
            self
        }
    }

    #[async_trait]
    impl AnalysisExecutor for FixtureExecutor {
        fn job_key(&self, skill_id: &str) -> Result<Option<String>, AnalysisServiceError> {
            self.keys
                .get(skill_id)
                .cloned()
                .ok_or(AnalysisServiceError {
                    code: AnalysisServiceErrorCode::SkillUnavailable,
                    message: "fixture unavailable",
                })
        }

        async fn execute(
            &self,
            skill_id: &str,
            _force: bool,
        ) -> Result<AnalysisResult, AnalysisServiceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(running, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.running.fetch_sub(1, Ordering::SeqCst);
            Ok(AnalysisResult {
                skill_id: skill_id.to_owned(),
                analysis_key: self.keys.get(skill_id).cloned().flatten(),
                status: if self.failed_skill.as_deref() == Some(skill_id) {
                    AnalysisRunStatus::Failed
                } else {
                    AnalysisRunStatus::Ready
                },
                passport: None,
                provider: Some("fixture".to_owned()),
                model: Some("fixture".to_owned()),
                cache_hit: false,
                attempts: 1,
                redactions: RedactionCounts::default(),
                sent_sections: Vec::new(),
                diagnostics: Vec::new(),
            })
        }
    }

    #[derive(Default)]
    struct FixtureSink {
        events: Mutex<Vec<AnalysisProgress>>,
    }

    impl AnalysisProgressSink for FixtureSink {
        fn emit(&self, progress: &AnalysisProgress) {
            self.events.lock().unwrap().push(progress.clone());
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    async fn wait_for_terminal(queue: &AnalysisQueue, expected: usize) {
        for _ in 0..200 {
            let progress = queue.progress();
            if progress.queued == 0 && progress.running == 0 && progress.total == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("queue did not reach a terminal state");
    }

    #[test]
    fn enqueue_returns_before_a_slow_analysis_finishes() {
        runtime().block_on(async {
            let executor = Arc::new(FixtureExecutor::new(1, Duration::from_millis(100)));
            let queue = AnalysisQueue::with_executor(executor, 2, Arc::new(FixtureSink::default()));
            let started = Instant::now();
            let result = queue.enqueue("skill-0".to_owned(), false).unwrap();

            assert_eq!(result.status, AnalysisJobStatus::Queued);
            assert!(started.elapsed() < Duration::from_millis(50));
            wait_for_terminal(&queue, 1).await;
        });
    }

    #[test]
    fn duplicate_analysis_keys_share_one_inflight_job() {
        runtime().block_on(async {
            let executor =
                Arc::new(FixtureExecutor::new(2, Duration::from_millis(20)).with_duplicate_key());
            let queue = AnalysisQueue::with_executor(
                Arc::clone(&executor) as Arc<dyn AnalysisExecutor>,
                2,
                Arc::new(FixtureSink::default()),
            );
            let first = queue.enqueue("skill-0".to_owned(), false).unwrap();
            let second = queue.enqueue("skill-1".to_owned(), false).unwrap();
            wait_for_terminal(&queue, 1).await;

            assert!(!first.deduplicated);
            assert!(second.deduplicated);
            assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn requested_concurrency_is_clamped_to_four() {
        runtime().block_on(async {
            let executor = Arc::new(FixtureExecutor::new(10, Duration::from_millis(20)));
            let queue = AnalysisQueue::with_executor(
                Arc::clone(&executor) as Arc<dyn AnalysisExecutor>,
                10,
                Arc::new(FixtureSink::default()),
            );
            queue.enqueue_many(
                (0..10).map(|index| format!("skill-{index}")).collect(),
                false,
            );
            wait_for_terminal(&queue, 10).await;

            assert!(executor.maximum.load(Ordering::SeqCst) <= 4);
            assert!(executor.maximum.load(Ordering::SeqCst) >= 2);
        });
    }

    #[test]
    fn one_failed_job_does_not_block_other_jobs() {
        runtime().block_on(async {
            let executor = Arc::new(
                FixtureExecutor::new(3, Duration::from_millis(5)).with_failed_skill("skill-1"),
            );
            let queue = AnalysisQueue::with_executor(executor, 2, Arc::new(FixtureSink::default()));
            queue.enqueue_many(
                vec![
                    "skill-0".to_owned(),
                    "skill-1".to_owned(),
                    "skill-2".to_owned(),
                ],
                false,
            );
            wait_for_terminal(&queue, 3).await;
            let progress = queue.progress();

            assert_eq!(progress.failed, 1);
            assert_eq!(progress.completed, 2);
        });
    }

    #[test]
    fn progress_events_never_include_analysis_content() {
        runtime().block_on(async {
            let executor = Arc::new(FixtureExecutor::new(1, Duration::from_millis(5)));
            let sink = Arc::new(FixtureSink::default());
            let queue = AnalysisQueue::with_executor(
                executor,
                2,
                Arc::clone(&sink) as Arc<dyn AnalysisProgressSink>,
            );
            queue.enqueue("skill-0".to_owned(), false).unwrap();
            wait_for_terminal(&queue, 1).await;
            let encoded = serde_json::to_string(&sink.events.lock().unwrap().clone()).unwrap();

            assert!(encoded.contains("analysis-job:key-0"));
            assert!(!encoded.contains("content"));
            assert!(!encoded.contains("/Users/"));
        });
    }

    #[test]
    fn unconfigured_jobs_return_immediately_without_spawning() {
        runtime().block_on(async {
            let mut executor = FixtureExecutor::new(1, Duration::from_millis(5));
            executor.keys.insert("skill-0".to_owned(), None);
            let executor = Arc::new(executor);
            let queue = AnalysisQueue::with_executor(
                Arc::clone(&executor) as Arc<dyn AnalysisExecutor>,
                2,
                Arc::new(FixtureSink::default()),
            );
            let result = queue.enqueue("skill-0".to_owned(), false).unwrap();

            assert_eq!(
                result,
                AnalysisEnqueueResult {
                    job_id: None,
                    status: AnalysisJobStatus::NotConfigured,
                    deduplicated: false,
                }
            );
            assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
            assert_eq!(queue.progress().total, 0);
        });
    }
}
