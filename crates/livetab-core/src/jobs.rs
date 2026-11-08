/*
===============================================================================
Division By Zeros (DBZS) - Job-Orchestrator
Modul: jobs
Zweck: Abbruchfähige Hintergrundjobs mit typisiertem Status, Fortschritt,
       Abschluss-/Fehlerereignissen und Worker-unabhängiger Kommunikation.
Sprache: Rust
Hinweise:
- Längere Lyrics-/Deep-/Exportjobs laufen nie auf dem UI- oder Audio-Callback-Thread.
- Workerfehler werden als Events transportiert und nicht verschluckt.
===============================================================================
*/
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Kategorie eines Hintergrundjobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    DeepAnalysis,
    Lyrics,
    StemSeparation,
    TabGeneration,
    Export,
    Recording,
    ModelLoad,
    ProjectImport,
}

/// Lebenszyklusstatus eines Jobs.
#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// Aktueller Jobstand für Oberfläche, Tests und Dev Observer.
#[derive(Debug, Clone, PartialEq)]
pub struct JobStatus {
    pub id: String,
    pub kind: JobKind,
    pub state: JobState,
    pub progress: f32,
    pub message: String,
    pub elapsed_ms: u128,
}

/// Events, die ein Worker bei Fortschritt, Ergebnis, Fehler oder Abbruch meldet.
#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Started(JobStatus),
    Progress(JobStatus),
    Completed(JobStatus),
    Failed(JobStatus),
    Cancelled(JobStatus),
}

/// Abbruchsignal mit thread-sicherem atomarem Zustand.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Zweck: Fordert den kooperativen Abbruch eines Hintergrundjobs an.
    /// Eingaben: `self` – gemeinsames Abbruchsignal.
    /// Ausgabe: Keine Rückgabe; nachfolgende Tokenprüfungen liefern `true`.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Zweck: Prüft, ob ein Aufrufer oder Nutzer den Job abgebrochen hat.
    /// Eingaben: `self` – gemeinsames Abbruchsignal.
    /// Ausgabe: `true`, wenn Abbruch angefordert wurde.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Zweck: Liefert einen Fehler, wenn ein Workflow vor einem teuren Schritt abbrechen soll.
    /// Eingaben: `self` – Abbruchsignal.
    /// Ausgabe: `Ok(())` oder eine verständliche Abbruchmeldung.
    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err("Job wurde abgebrochen.".into())
        } else {
            Ok(())
        }
    }
}

/// Handle für Nutzeraktionen, die einen Job abbrechen dürfen.
#[derive(Debug, Clone)]
pub struct JobHandle {
    pub id: String,
    pub cancellation: CancellationToken,
}

/// Progressreporter, den ein Worker innerhalb seiner Funktion verwenden kann.
#[derive(Debug, Clone)]
pub struct JobReporter {
    id: String,
    kind: JobKind,
    started: Instant,
    sender: Sender<JobEvent>,
    statuses: Arc<Mutex<BTreeMap<String, JobStatus>>>,
}

impl JobReporter {
    /// Zweck: Meldet einen endlichen Jobfortschritt und aktualisiert den abrufbaren Status.
    /// Eingaben: `progress` – Anteil 0 bis 1; `message` – kurze Nutzer-/Observermeldung.
    /// Ausgabe: Keine Rückgabe; sendet ein Fortschrittsereignis.
    pub fn progress(&self, progress: f32, message: impl Into<String>) {
        let status = JobStatus {
            id: self.id.clone(),
            kind: self.kind,
            state: JobState::Running,
            progress: if progress.is_finite() {
                progress.clamp(0.0, 1.0)
            } else {
                0.0
            },
            message: message.into(),
            elapsed_ms: self.started.elapsed().as_millis(),
        };
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.insert(self.id.clone(), status.clone());
        }
        let _ = self.sender.send(JobEvent::Progress(status));
    }

    /// Zweck: Liefert die stabile Kennung des laufenden Jobs für Event- und Logkorrelation.
    /// Eingaben: `self` – Reporter.
    /// Ausgabe: Unveränderliche ID-Zeichenkette.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Threadbasierter Manager für gleichzeitig laufende Deep-, Lyrics-, Tab- und Exportjobs.
#[derive(Debug)]
pub struct JobManager {
    next_id: AtomicU64,
    sender: Sender<JobEvent>,
    receiver: Mutex<Option<Receiver<JobEvent>>>,
    statuses: Arc<Mutex<BTreeMap<String, JobStatus>>>,
}

impl JobManager {
    /// Zweck: Erstellt einen Jobmanager und reserviert seinen Eventempfänger.
    /// Eingaben: Keine.
    /// Ausgabe: Manager mit leeren Statusdaten und gemeinsamem Eventkanal.
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            next_id: AtomicU64::new(1),
            sender,
            receiver: Mutex::new(Some(receiver)),
            statuses: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Zweck: Überträgt den Eventempfänger einmalig an den UI-/Controller-Dispatcher.
    /// Eingaben: `self` – Manager.
    /// Ausgabe: Kanalempfänger oder `None`, wenn er bereits entnommen wurde.
    pub fn take_event_receiver(&self) -> Option<Receiver<JobEvent>> {
        self.receiver
            .lock()
            .ok()
            .and_then(|mut receiver| receiver.take())
    }

    /// Zweck: Startet eine Workerfunktion auf einem eigenen Thread und erfasst ihren Lebenszyklus.
    /// Eingaben: Jobtyp; `work` erhält Reporter und Abbruchtoken und liefert Erfolgstext oder Fehler.
    /// Ausgabe: `JobHandle`, mit dem der gestartete Job kooperativ abgebrochen werden kann.
    pub fn start<F>(&self, kind: JobKind, work: F) -> JobHandle
    where
        F: FnOnce(JobReporter, CancellationToken) -> Result<String, String> + Send + 'static,
    {
        let id = format!("job-{:08}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let token = CancellationToken::default();
        let worker_token = token.clone();
        let sender = self.sender.clone();
        let statuses = Arc::clone(&self.statuses);
        let worker_sender = sender.clone();
        let worker_statuses = Arc::clone(&statuses);
        let job_id = id.clone();
        let queued = JobStatus {
            id: id.clone(),
            kind,
            state: JobState::Queued,
            progress: 0.0,
            message: "Job wartet auf Ausführung.".into(),
            elapsed_ms: 0,
        };
        if let Ok(mut all) = statuses.lock() {
            all.insert(id.clone(), queued.clone());
        }
        let _ = sender.send(JobEvent::Started(queued));
        std::thread::spawn(move || {
            let started = Instant::now();
            let reporter = JobReporter {
                id: job_id.clone(),
                kind,
                started,
                sender: worker_sender.clone(),
                statuses: Arc::clone(&worker_statuses),
            };
            let running = JobStatus {
                id: job_id.clone(),
                kind,
                state: JobState::Running,
                progress: 0.0,
                message: "Job gestartet.".into(),
                elapsed_ms: 0,
            };
            if let Ok(mut all) = worker_statuses.lock() {
                all.insert(job_id.clone(), running.clone());
            }
            let _ = worker_sender.send(JobEvent::Progress(running));
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                work(reporter, worker_token.clone())
            }));
            let (state, progress, message) = if worker_token.is_cancelled() {
                (JobState::Cancelled, 0.0, "Job abgebrochen.".into())
            } else {
                match outcome {
                    Ok(Ok(message)) => (JobState::Succeeded, 1.0, message),
                    Ok(Err(message)) => (JobState::Failed, 0.0, message),
                    Err(_) => (
                        JobState::Failed,
                        0.0,
                        "Job ist durch einen internen Workerfehler fehlgeschlagen.".into(),
                    ),
                }
            };
            let status = JobStatus {
                id: job_id.clone(),
                kind,
                state: state.clone(),
                progress,
                message,
                elapsed_ms: started.elapsed().as_millis(),
            };
            if let Ok(mut all) = worker_statuses.lock() {
                all.insert(job_id, status.clone());
            }
            let event = match state {
                JobState::Succeeded => JobEvent::Completed(status),
                JobState::Failed => JobEvent::Failed(status),
                JobState::Cancelled => JobEvent::Cancelled(status),
                _ => return,
            };
            let _ = worker_sender.send(event);
        });
        JobHandle {
            id,
            cancellation: token,
        }
    }

    /// Zweck: Liefert einen einzelnen aktuellen Jobstatus.
    /// Eingaben: `id` – stabile Jobkennung.
    /// Ausgabe: Statuskopie oder `None`, wenn der Job nicht bekannt ist.
    pub fn status(&self, id: &str) -> Option<JobStatus> {
        self.statuses.lock().ok()?.get(id).cloned()
    }

    /// Zweck: Listet alle bekannten Jobs nach stabiler Kennung.
    /// Eingaben: `self` – Jobmanager.
    /// Ausgabe: Sortierte Statuskopien für Observer und Diagnose.
    pub fn statuses(&self) -> Vec<JobStatus> {
        self.statuses
            .lock()
            .map(|all| all.values().cloned().collect())
            .unwrap_or_default()
    }
}

impl Default for JobManager {
    /// Zweck: Verwendet dieselbe Initialisierung wie `new`.
    /// Eingaben: Keine.
    /// Ausgabe: Leerer Jobmanager.
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Zweck: Prüft Fortschrittsmeldungen, Ergebnisstatus und Workerfehlerfang.
    /// Eingaben: Keine; startet einen erfolgreichen und einen fehlschlagenden Testjob.
    /// Ausgabe: Keine Rückgabe; Assertions erwarten terminale Zustände und sichtbaren Fehlertext.
    #[test]
    fn records_job_progress_and_failure() {
        let manager = JobManager::new();
        let receiver = manager.take_event_receiver().unwrap();
        let handle = manager.start(JobKind::Export, |reporter, _| {
            reporter.progress(0.5, "Datei schreiben");
            Ok("Export fertig".into())
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut completed = false;
        while Instant::now() < deadline {
            if let Ok(JobEvent::Completed(status)) =
                receiver.recv_timeout(Duration::from_millis(50))
            {
                if status.id == handle.id {
                    assert_eq!(status.progress, 1.0);
                    completed = true;
                    break;
                }
            }
        }
        assert!(completed);
        assert_eq!(
            manager.status(&handle.id).unwrap().state,
            JobState::Succeeded
        );
        let failed = manager.start(JobKind::Lyrics, |_, _| Err("Modell fehlt".into()));
        let mut failure_seen = false;
        while Instant::now() < deadline + Duration::from_secs(2) {
            if let Ok(JobEvent::Failed(status)) = receiver.recv_timeout(Duration::from_millis(50)) {
                if status.id == failed.id {
                    assert!(status.message.contains("Modell fehlt"));
                    failure_seen = true;
                    break;
                }
            }
        }
        assert!(failure_seen);
    }

    /// Zweck: Prüft, dass Abbruchanforderungen im Worker kooperativ sichtbar sind.
    /// Eingaben: Keine; startet einen kurz wartenden Job und bricht ihn sofort ab.
    /// Ausgabe: Keine Rückgabe; finaler Jobzustand muss `Cancelled` sein.
    #[test]
    fn cancellation_is_reported_without_silent_success() {
        let manager = JobManager::new();
        let receiver = manager.take_event_receiver().unwrap();
        let handle = manager.start(JobKind::DeepAnalysis, |_, token| {
            while !token.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok("unreachable".into())
        });
        handle.cancellation.cancel();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut cancelled = false;
        while Instant::now() < deadline {
            if let Ok(JobEvent::Cancelled(status)) =
                receiver.recv_timeout(Duration::from_millis(50))
            {
                if status.id == handle.id {
                    cancelled = true;
                    break;
                }
            }
        }
        assert!(cancelled);
    }
}
