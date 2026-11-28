//! The sidecar decorator around a compose runner: the real tmpfs store, a fake state probe, a fake
//! controller and a fake runner that reads what Compose would see while it runs.

use std::future::{Future, ready};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aegis_adapters::allowlist;
use aegis_adapters::{SidecarCompose, SidecarPorts, TmpfsStore};
use aegis_application::{
    AuthKey, BuiltServices, ComposeError, ComposeRunner, DeployRequest, EnrollError, Enrolled,
    EnrollmentRequest, Exposure, Grant, Reason, SidecarEnroller, StateProbe, StateVolume, Target,
};
use aegis_domain::{
    CommitSha, Expose, Port, ProjectName, RepoPath, ServiceName, StackName, TailnetName,
    TailnetTag, TailnetTags,
};
use serde_norway::Value;

const KEY: &str = "tskey-auth-minted-secret";

#[cfg(test)]
struct Probe(Result<StateVolume, &'static str>);

#[cfg(test)]
impl StateProbe for Probe {
    fn probe(
        &self,
        _project: &ProjectName,
    ) -> impl Future<Output = Result<StateVolume, Reason>> + Send {
        ready(self.0.map_err(|text| Reason::new(&text)))
    }
}

/// Answers like a controller and remembers what it was asked.
#[cfg(test)]
struct Desk {
    answer: Result<Enrolled, &'static str>,
    asked: Arc<Mutex<Vec<EnrollmentRequest>>>,
}

#[cfg(test)]
impl SidecarEnroller for Desk {
    fn enroll(
        &self,
        request: &EnrollmentRequest,
    ) -> impl Future<Output = Result<Grant, EnrollError>> + Send {
        self.asked.lock().unwrap().push(request.clone());
        let tailnet = TailnetName::new("taildfab0.ts.net").unwrap();
        ready(match &self.answer {
            Ok(enrolled) => Ok(Grant::new(tailnet, enrolled.clone())),
            Err(text) => Err(EnrollError::Controller(Reason::new(text))),
        })
    }
}

/// What the runner saw while it ran.
#[derive(Debug, Clone)]
#[cfg(test)]
struct Seen {
    override_text: String,
    key_variable: Option<String>,
    files: Vec<PathBuf>,
}

#[cfg(test)]
struct Runner {
    failure: Option<&'static str>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[cfg(test)]
impl ComposeRunner for Runner {
    async fn converge(&self, request: &DeployRequest) -> Result<(), ComposeError> {
        let files: Vec<PathBuf> = request
            .extras()
            .files()
            .as_slice()
            .iter()
            .map(|file| PathBuf::from(file.as_str()))
            .collect();
        let text = read_all(&files).await;
        let variables = request.extras().environment().variables();
        let key_variable = variables.get("AEGIS_TS_AUTHKEY").cloned();
        let seen = Seen {
            override_text: text,
            key_variable,
            files,
        };
        self.seen.lock().unwrap().push(seen);
        self.failure
            .map_or(Ok(()), |text| Err(ComposeError::new(Reason::new(&text))))
    }
}

#[cfg(test)]
struct Harness {
    compose: SidecarCompose<Runner, Probe, Desk, TmpfsStore>,
    seen: Arc<Mutex<Vec<Seen>>>,
    asked: Arc<Mutex<Vec<EnrollmentRequest>>>,
    store: TmpfsStore,
}

#[cfg(test)]
fn harness(
    name: &str,
    probe: Result<StateVolume, &'static str>,
    answer: Result<Enrolled, &'static str>,
) -> Harness {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let root =
        PathBuf::from("/dev/shm").join(format!("aegis-sidecar-{}-{name}", std::process::id()));
    let store = TmpfsStore::at(root);
    let runner = Runner {
        failure: None,
        seen: seen.clone(),
    };
    let desk = Desk {
        answer,
        asked: asked.clone(),
    };
    let ports = SidecarPorts::new(Probe(probe), desk, store.clone());
    Harness {
        compose: SidecarCompose::new(runner, ports),
        seen,
        asked,
        store,
    }
}

#[cfg(test)]
fn exposure() -> Exposure {
    let service = ServiceName::new("vaultwarden").unwrap();
    let tags: TailnetTags = std::iter::once(TailnetTag::new("tag:app").unwrap()).collect();
    Exposure::new(Expose::new(service, Port::new(80).unwrap()), tags)
}

#[cfg(test)]
fn request(exposure: Option<Exposure>) -> DeployRequest {
    let project = ProjectName::of(&StackName::new("vaultwarden").unwrap()).unwrap();
    let target = Target::new(project, RepoPath::new("hosts/host/vaultwarden").unwrap());
    let commit = CommitSha::new("abc1234").unwrap();
    DeployRequest::new(
        target.with_exposure(exposure),
        commit,
        BuiltServices::default(),
    )
}

#[cfg(test)]
fn minted() -> Enrolled {
    Enrolled::Minted(AuthKey::new(KEY))
}

#[cfg(test)]
async fn read(file: &PathBuf) -> String {
    tokio::fs::read_to_string(file).await.unwrap()
}

#[cfg(test)]
async fn read_all(files: &[PathBuf]) -> String {
    let mut text = String::new();
    for file in files {
        text.push_str(&read(file).await);
    }
    text
}

#[cfg(test)]
async fn exists(file: &PathBuf) -> bool {
    tokio::fs::try_exists(file).await.unwrap()
}

#[cfg(test)]
async fn workspace_is_gone(files: &[PathBuf]) -> bool {
    let mut found = Vec::new();
    for file in files {
        found.push(exists(file).await);
    }
    found.iter().all(|present| !present)
}

#[test]
fn a_stack_that_exposes_nothing_is_run_untouched() {
    runtime().block_on(async {
        let rig = harness("plain", Ok(StateVolume::Empty), Ok(minted()));
        rig.compose.converge(&request(None)).await.unwrap();
        let seen = rig.seen.lock().unwrap().clone();
        assert!(seen[0].files.is_empty() && seen[0].key_variable.is_none());
        assert!(rig.asked.lock().unwrap().is_empty());
    });
}

#[test]
fn a_new_node_gets_the_override_and_its_key_only_in_the_environment() {
    runtime().block_on(async {
        let rig = harness("fresh", Ok(StateVolume::Empty), Ok(minted()));
        rig.compose
            .converge(&request(Some(exposure())))
            .await
            .unwrap();
        let seen = rig.seen.lock().unwrap().clone();
        assert_eq!(seen[0].key_variable.as_deref(), Some(KEY));
        assert!(seen[0].override_text.contains("TS_HOSTNAME: vaultwarden"));
        assert!(
            seen[0]
                .override_text
                .contains("vaultwarden.taildfab0.ts.net {")
        );
        assert!(!seen[0].override_text.contains(KEY));
        let asked = rig.asked.lock().unwrap().clone();
        assert_eq!(asked[0].state(), StateVolume::Empty);
        assert_eq!(asked[0].node().as_str(), "vaultwarden");
        assert!(workspace_is_gone(&seen[0].files).await);
    });
}

#[test]
fn a_known_node_is_deployed_without_a_key() {
    runtime().block_on(async {
        let rig = harness(
            "known",
            Ok(StateVolume::Populated),
            Ok(Enrolled::AlreadyEnrolled),
        );
        rig.compose
            .converge(&request(Some(exposure())))
            .await
            .unwrap();
        let seen = rig.seen.lock().unwrap().clone();
        assert!(seen[0].key_variable.is_none());
        assert!(!seen[0].override_text.contains("TS_AUTHKEY"));
        let asked = rig.asked.lock().unwrap().clone();
        assert_eq!(asked[0].state(), StateVolume::Populated);
    });
}

#[test]
fn the_rendered_override_is_valid_yaml_with_no_tags() {
    runtime().block_on(async {
        let rig = harness("yaml", Ok(StateVolume::Empty), Ok(minted()));
        rig.compose
            .converge(&request(Some(exposure())))
            .await
            .unwrap();
        let seen = rig.seen.lock().unwrap().clone();
        let document: Value = serde_norway::from_str(&seen[0].override_text).unwrap();
        assert!(allowlist::tags(&document).is_empty());
        let caddy = &document["services"]["caddy"];
        assert_eq!(caddy["network_mode"], Value::from("service:tailscale"));
        assert!(document["configs"]["aegis-caddyfile"]["content"].is_string());
    });
}

#[test]
fn an_unreachable_controller_fails_the_stack_before_compose_runs() {
    runtime().block_on(async {
        let rig = harness("down", Ok(StateVolume::Empty), Err("503"));
        let error = rig
            .compose
            .converge(&request(Some(exposure())))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("cannot enrol the sidecar"),
            "{error}"
        );
        assert!(rig.seen.lock().unwrap().is_empty());
    });
}

#[test]
fn an_unreadable_state_volume_fails_the_stack_and_enrols_nothing() {
    runtime().block_on(async {
        let rig = harness("probe", Err("docker is down"), Ok(minted()));
        let error = rig
            .compose
            .converge(&request(Some(exposure())))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("cannot check the sidecar state"),
            "{error}"
        );
        assert!(rig.asked.lock().unwrap().is_empty());
    });
}

#[test]
fn a_failing_compose_still_removes_the_override() {
    runtime().block_on(async {
        let mut rig = harness("boom", Ok(StateVolume::Empty), Ok(minted()));
        rig.compose = SidecarCompose::new(
            Runner {
                failure: Some("compose failed"),
                seen: rig.seen.clone(),
            },
            SidecarPorts::new(
                Probe(Ok(StateVolume::Empty)),
                Desk {
                    answer: Ok(minted()),
                    asked: rig.asked.clone(),
                },
                rig.store.clone(),
            ),
        );
        let error = rig
            .compose
            .converge(&request(Some(exposure())))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("compose failed"));
        let seen = rig.seen.lock().unwrap().clone();
        assert!(workspace_is_gone(&seen[0].files).await);
    });
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
