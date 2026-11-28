//! A request for a node must match what the desired state says the calling host runs.

#[cfg(test)]
mod cases {
    use aegis_application::{
        EnrollmentRequest, Misplaced, Outcome, SidecarIdentity, StateVolume, check_enrollment,
        knows_stack, purges_node,
    };
    use aegis_domain::{
        ComposeSource, ComposeSummary, DesiredState, Expose, Host, HostName, Hosts, NodeLabel,
        Port, Rejection, Rejections, RepoPath, ServiceName, Services, Stack, StackDefinition,
        StackIdentity, StackKind, StackName, StackPolicy, StackState, Stacks, TailnetTag,
        TailnetTags, Violations, Volumes,
    };

    fn tags(names: &[&str]) -> TailnetTags {
        let tags = names.iter().map(|name| TailnetTag::new(*name).unwrap());
        tags.collect()
    }

    /// A policy that exposes `service` with the tags `declared`.
    fn policy(service: Option<&str>, declared: &[&str], state: StackState) -> StackPolicy {
        let expose = service
            .map(|text| Expose::new(ServiceName::new(text).unwrap(), Port::new(8080).unwrap()));
        StackPolicy::new(expose, tags(declared), state)
    }

    fn stack(name: &str, policy: StackPolicy) -> Stack {
        let compose = ComposeSummary::new(Services::default(), Volumes::default());
        let definition = StackDefinition::new(policy, ComposeSource::new(""), compose);
        let identity = StackIdentity::new(StackName::new(name).unwrap(), StackKind::Host);
        Stack::new(identity, definition)
    }

    fn host(name: &str, stacks: Vec<Stack>) -> Host {
        Host::new(HostName::new(name).unwrap(), Stacks::new(stacks))
    }

    fn desired(hosts: Vec<Host>) -> DesiredState {
        DesiredState::new(Hosts::new(hosts), Stacks::default())
    }

    fn name(text: &str) -> HostName {
        HostName::new(text).unwrap()
    }

    fn request(node: &str, asked: &[&str]) -> EnrollmentRequest {
        let identity = SidecarIdentity::new(
            StackName::new("vault").unwrap(),
            NodeLabel::new(node).unwrap(),
        );
        EnrollmentRequest::new(identity, tags(asked), StateVolume::Empty)
    }

    fn vault(service: Option<&str>, declared: &[&str], state: StackState) -> DesiredState {
        desired(vec![host(
            "h",
            vec![stack("vault", policy(service, declared, state))],
        )])
    }

    #[test]
    fn an_active_exposed_stack_with_the_same_node_and_tags_is_placed() {
        let state = vault(Some("vaultwarden"), &["tag:app"], StackState::Active);
        let asked = request("vaultwarden", &["tag:app"]);
        assert_eq!(check_enrollment(&state, &name("h"), &asked), Ok(()));
    }

    fn raw(service: &str, state: StackState) -> StackPolicy {
        let node = aegis_domain::Node::new(ServiceName::new(service).unwrap());
        StackPolicy::new(None, tags(&["tag:app"]), state).with_node(node)
    }

    #[test]
    fn an_active_stack_with_a_raw_node_of_that_hostname_is_placed() {
        let state = desired(vec![host(
            "h",
            vec![stack("vault", raw("vault", StackState::Active))],
        )]);
        assert_eq!(
            check_enrollment(&state, &name("h"), &request("vault", &["tag:app"])),
            Ok(())
        );
        assert_eq!(
            check_enrollment(&state, &name("h"), &request("other", &["tag:app"])),
            Err(Misplaced::NotPlaced)
        );
        assert_eq!(
            check_enrollment(&state, &name("h"), &request("vault", &[])),
            Err(Misplaced::WrongTags)
        );
    }

    #[test]
    fn a_purged_stack_with_a_raw_node_owns_the_node_for_deletion() {
        let purged = stack("vault", raw("vault", StackState::Purged));
        let node = NodeLabel::new("vault").unwrap();
        let gone = outcome(vec![host("h", vec![purged])], vec![]);
        assert!(purges_node(&gone, &name("h"), &node));
        let held = stack("app", raw("vault", StackState::Active));
        let kept = outcome(
            vec![host(
                "h",
                vec![stack("vault", raw("vault", StackState::Purged)), held],
            )],
            vec![],
        );
        assert!(!purges_node(&kept, &name("h"), &node));
    }

    #[test]
    fn the_order_of_the_tags_does_not_matter() {
        let state = vault(Some("vaultwarden"), &["tag:a", "tag:b"], StackState::Active);
        let asked = request("vaultwarden", &["tag:b", "tag:a"]);
        assert_eq!(check_enrollment(&state, &name("h"), &asked), Ok(()));
    }

    #[test]
    fn a_stack_the_host_does_not_have_is_not_placed_even_when_another_host_has_it() {
        let state = desired(vec![
            host("h", Vec::new()),
            host(
                "g",
                vec![stack(
                    "vault",
                    policy(Some("vaultwarden"), &["tag:app"], StackState::Active),
                )],
            ),
        ]);
        let asked = request("vaultwarden", &["tag:app"]);
        let verdict = check_enrollment(&state, &name("h"), &asked);
        assert_eq!(verdict, Err(Misplaced::NotPlaced));
        assert!(!knows_stack(&state, &name("h"), asked.stack()));
        assert!(knows_stack(&state, &name("g"), asked.stack()));
    }

    #[test]
    fn a_stack_that_exposes_nothing_is_not_placed() {
        let state = vault(None, &["tag:app"], StackState::Active);
        let asked = request("vaultwarden", &["tag:app"]);
        let verdict = check_enrollment(&state, &name("h"), &asked);
        assert_eq!(verdict, Err(Misplaced::NotPlaced));
    }

    #[test]
    fn a_hostname_other_than_the_exposed_service_is_not_placed() {
        let state = vault(Some("vaultwarden"), &["tag:app"], StackState::Active);
        let asked = request("somebody-else", &["tag:app"]);
        let verdict = check_enrollment(&state, &name("h"), &asked);
        assert_eq!(verdict, Err(Misplaced::NotPlaced));
    }

    #[test]
    fn a_purged_stack_is_not_placed() {
        let state = vault(Some("vaultwarden"), &["tag:app"], StackState::Purged);
        let asked = request("vaultwarden", &["tag:app"]);
        let verdict = check_enrollment(&state, &name("h"), &asked);
        assert_eq!(verdict, Err(Misplaced::NotPlaced));
    }

    #[test]
    fn an_extra_or_a_missing_tag_is_wrong() {
        let state = vault(Some("vaultwarden"), &["tag:app"], StackState::Active);
        let extra = request("vaultwarden", &["tag:app", "tag:admin"]);
        let missing = request("vaultwarden", &[]);
        let wrong = Err(Misplaced::WrongTags);
        assert_eq!(check_enrollment(&state, &name("h"), &extra), wrong);
        assert_eq!(check_enrollment(&state, &name("h"), &missing), wrong);
    }

    fn old(service: &str, state: StackState) -> Stack {
        stack("old", policy(Some(service), &[], state))
    }

    fn outcome(hosts: Vec<Host>, reserved: Vec<Stack>) -> Outcome {
        let state = DesiredState::new(Hosts::new(hosts), Stacks::new(reserved));
        Outcome::new(state, Rejections::default())
    }

    fn rejecting(hosts: Vec<Host>) -> Outcome {
        let state = DesiredState::new(Hosts::new(hosts), Stacks::default());
        let broken = Rejection::new(
            RepoPath::new("hosts/g/broken").unwrap(),
            Violations::default(),
        );
        Outcome::new(state, Rejections::new(vec![broken]))
    }

    #[test]
    fn only_a_purged_stack_of_the_callers_host_owns_its_node_for_deletion() {
        let node = NodeLabel::new("vault").unwrap();
        let purged = outcome(
            vec![host("h", vec![old("vault", StackState::Purged)])],
            vec![],
        );
        let active = outcome(
            vec![host("h", vec![old("vault", StackState::Active)])],
            vec![],
        );
        let other = outcome(
            vec![host("h", vec![old("other", StackState::Purged)])],
            vec![],
        );
        let elsewhere = outcome(
            vec![
                host("h", Vec::new()),
                host("g", vec![old("vault", StackState::Purged)]),
            ],
            vec![],
        );
        assert!(purges_node(&purged, &name("h"), &node));
        assert!(!purges_node(&active, &name("h"), &node));
        assert!(!purges_node(&other, &name("h"), &node));
        assert!(!purges_node(&elsewhere, &name("h"), &node));
    }

    #[test]
    fn a_node_name_another_host_or_a_reserved_stack_still_uses_is_never_deleted() {
        let node = NodeLabel::new("vault").unwrap();
        let mine = host("h", vec![old("vault", StackState::Purged)]);
        let theirs = stack("app", policy(Some("vault"), &[], StackState::Active));
        let moved = outcome(vec![mine.clone(), host("g", vec![theirs.clone()])], vec![]);
        let reserved = outcome(vec![mine], vec![theirs]);
        assert!(!purges_node(&moved, &name("h"), &node));
        assert!(!purges_node(&reserved, &name("h"), &node));
    }

    #[test]
    fn no_node_is_deleted_while_a_stack_is_rejected_because_it_may_still_run_with_the_name() {
        let node = NodeLabel::new("vault").unwrap();
        let mine = host("h", vec![old("vault", StackState::Purged)]);
        assert!(!purges_node(&rejecting(vec![mine]), &name("h"), &node));
    }
}
