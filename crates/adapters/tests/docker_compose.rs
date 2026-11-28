//! The `docker compose` adapter against a fake `docker` executable that records how it was run.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use aegis_adapters::DockerCompose;
use aegis_application::{
    BuiltServices, ComposeRunner, DeployRequest, Environment, ExtraFiles, Extras, Target,
};
use aegis_domain::{CommitSha, HostPath, ProjectName, RepoPath, ServiceName, StackName};

/// A scratch directory with a fake `docker` in it and the stack directory it will run in.
#[cfg(test)]
struct Sandbox {
    root: PathBuf,
}

#[cfg(test)]
impl Sandbox {
    async fn new(name: &str, exit_code: i32) -> Self {
        let root = std::env::temp_dir().join(format!("aegis-{}-{name}", std::process::id()));
        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::create_dir_all(root.join("hosts/host/web"))
            .await
            .unwrap();
        let stack = root.join("hosts/host/web/compose.override.yaml");
        tokio::fs::write(stack, "services: {}\n").await.unwrap();
        let sandbox = Self { root };
        sandbox.install_fake(exit_code).await;
        sandbox
    }

    fn log(&self) -> PathBuf {
        self.root.join("log")
    }

    fn fake(&self) -> PathBuf {
        self.root.join("fake-docker")
    }

    async fn install_fake(&self, exit_code: i32) {
        let script = format!(
            "#!/bin/sh\n{{\necho \"ARGS:$*\"\necho \"PWD:$(pwd)\"\necho \"COMPOSE_FILE:${{COMPOSE_FILE-unset}}\"\necho \"DOCKER_HOST:${{DOCKER_HOST-unset}}\"\necho \"ENVNAMES:$(env | cut -d= -f1 | sort | tr '\\n' ' ')\"\n\
             echo \"TOKEN:${{TOKEN-unset}}\"\necho \"STDIN:\"\ncat\n}} >> {log}\n\
             echo \"boom $TOKEN\" >&2\nexit {exit_code}\n",
            log = self.log().display()
        );
        tokio::fs::write(self.fake(), script).await.unwrap();
        let executable = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(self.fake(), executable)
            .await
            .unwrap();
    }

    async fn recorded(&self) -> String {
        tokio::fs::read_to_string(self.log()).await.unwrap()
    }

    fn runner(&self) -> DockerCompose {
        DockerCompose::with_program(self.fake().display().to_string(), &self.root)
    }

    async fn pwd(&self) -> String {
        let stack = self.root.join("hosts/host/web");
        let canonical = tokio::fs::canonicalize(stack).await.unwrap();
        canonical.display().to_string()
    }
}

#[cfg(test)]
fn request(built: &[&str]) -> DeployRequest {
    let project = ProjectName::of(&StackName::new("web").unwrap()).unwrap();
    let target = Target::new(project, RepoPath::new("hosts/host/web").unwrap());
    let services = built.iter().map(|name| ServiceName::new(*name).unwrap());
    let commit = CommitSha::new("abc1234").unwrap();
    DeployRequest::new(target, commit, services.collect::<BuiltServices>())
}

#[cfg(test)]
async fn run(runner: &DockerCompose, request: &DeployRequest) -> Result<(), String> {
    let outcome = runner.converge(request).await;
    outcome.map_err(|error| error.to_string())
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn a_stack_is_brought_up_under_its_project_with_only_its_own_compose_file() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("pinned", 0).await;
        run(&sandbox.runner(), &request(&[])).await.unwrap();
        let recorded = sandbox.recorded().await;
        assert!(recorded.contains("ARGS:compose -p aegis-web -f compose.yaml up -d --build\n"));
        assert!(!recorded.contains("override"));
        assert!(recorded.contains(&format!("PWD:{}", sandbox.pwd().await)));
    });
}

/// Runs only inside the child process the next test starts, where the parent has set variables
/// that must not reach Compose.
#[test]
fn inherited_environment_child() {
    let Ok(root) = std::env::var("AEGIS_CHILD_SANDBOX") else {
        return;
    };
    let sandbox = Sandbox {
        root: PathBuf::from(root),
    };
    runtime().block_on(async {
        run(&sandbox.runner(), &request(&[])).await.unwrap();
    });
}

#[test]
fn only_the_docker_socket_and_search_path_variables_reach_compose() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("env", 0).await;
        let socket = "unix:///run/user/1000/docker.sock";
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "inherited_environment_child"])
            .env("AEGIS_CHILD_SANDBOX", &sandbox.root)
            .env("DOCKER_HOST", socket)
            .env("COMPOSE_FILE", "evil.yaml")
            .env("COMPOSE_PROJECT_NAME", "evil")
            .status()
            .unwrap();
        let recorded = sandbox.recorded().await;
        assert!(status.success());
        assert!(recorded.contains(&format!("DOCKER_HOST:{socket}")));
        assert!(recorded.contains("COMPOSE_FILE:unset"));
        assert!(!recorded.contains("COMPOSE_PROJECT_NAME"));
        assert!(!recorded.contains("AEGIS_CHILD_SANDBOX"));
    });
}

#[test]
fn built_services_are_tagged_with_the_commit() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("built", 0).await;
        run(&sandbox.runner(), &request(&["App", "worker"]))
            .await
            .unwrap();
        let recorded = sandbox.recorded().await;
        assert!(recorded.contains("-f compose.yaml -f - up -d --build"));
        assert!(recorded.contains("\"aegis-web-app:abc1234\""));
        assert!(recorded.contains("\"aegis-web-worker:abc1234\""));
        assert!(recorded.contains("  App:\n    build:\n      tags:"));
    });
}

#[test]
fn extra_files_follow_the_stack_file_and_come_before_the_tags() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("extras", 0).await;
        let files = ExtraFiles::new(vec![HostPath::new("/var/lib/aegis/web.yaml").unwrap()]);
        let extras = Extras::new(files, Environment::default());
        let deploy = request(&["app"]).with_extras(extras);
        run(&sandbox.runner(), &deploy).await.unwrap();
        let expected = "-f compose.yaml -f /var/lib/aegis/web.yaml -f - up -d --build";
        assert!(sandbox.recorded().await.contains(expected));
    });
}

#[test]
fn injected_variables_reach_compose_and_stay_out_of_failures() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("secret", 3).await;
        let variables = BTreeMap::from([("TOKEN".to_owned(), "s3cret-value".to_owned())]);
        let extras = Extras::new(ExtraFiles::default(), Environment::new(variables));
        let deploy = request(&[]).with_extras(extras);
        let failure = run(&sandbox.runner(), &deploy).await.unwrap_err();
        assert!(sandbox.recorded().await.contains("TOKEN:s3cret-value"));
        assert!(failure.contains("boom ***"));
        assert!(!failure.contains("s3cret-value"));
    });
}

#[test]
fn a_failing_compose_is_an_error_and_a_missing_docker_is_too() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("fails", 1).await;
        let failure = run(&sandbox.runner(), &request(&[])).await.unwrap_err();
        assert!(failure.contains("exited with"));
        let missing = DockerCompose::with_program("/nonexistent/aegis-docker", &sandbox.root);
        assert!(run(&missing, &request(&[])).await.is_err());
    });
}

#[test]
fn a_decrypted_dotenv_is_passed_with_env_file_before_the_project() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("dotenv", 0).await;
        let dotenv = HostPath::new("/run/user/1000/aegis/aegis-web/dotenv").unwrap();
        let extras = Extras::default().with_dotenv(dotenv);
        let request = request(&[]).with_extras(extras);
        run(&sandbox.runner(), &request).await.unwrap();
        let recorded = sandbox.recorded().await;
        let expected = "ARGS:compose --env-file /run/user/1000/aegis/aegis-web/dotenv -p aegis-web";
        assert!(recorded.contains(expected), "{recorded}");
    });
}
