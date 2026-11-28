//! A request for a node must match what the desired state says the calling host runs.

#[cfg(test)]
mod cases {
    use aegis_application::{
        EnrollmentRequest, Misplaced, SidecarIdentity, StateVolume, check_enrollment,
        is_purged_node,
    };
    use aegis_domain::{
        ComposeSource, ComposeSummary, Expose, NodeLabel, Port, ServiceName, Services, Stack,
        StackDefinition, StackIdentity, StackKind, StackName, StackPolicy, StackState, TailnetTag,
        TailnetTags, Volumes,
    };

    fn tags(names: &[&str]) -> TailnetTags {
        let tags = names.iter().map(|name| TailnetTag::new(*name).unwrap());
        tags.collect()
    }

    fn stack(service: Option<&str>, declared: &[&str], state: StackState) -> Stack {
        let expose = service
            .map(|name| Expose::new(ServiceName::new(name).unwrap(), Port::new(8080).unwrap()));
        let policy = StackPolicy::new(expose, tags(declared), state);
        let compose = ComposeSummary::new(Services::default(), Volumes::default());
        let definition = StackDefinition::new(policy, ComposeSource::new(""), compose);
        let identity = StackIdentity::new(StackName::new("vault").unwrap(), StackKind::Host);
        Stack::new(identity, definition)
    }

    fn request(node: &str, asked: &[&str]) -> EnrollmentRequest {
        let identity = SidecarIdentity::new(
            StackName::new("vault").unwrap(),
            NodeLabel::new(node).unwrap(),
        );
        EnrollmentRequest::new(identity, tags(asked), StateVolume::Empty)
    }

    #[test]
    fn an_active_exposed_stack_with_the_same_node_and_tags_is_placed() {
        let placed = stack(Some("vaultwarden"), &["tag:app"], StackState::Active);
        let asked = request("vaultwarden", &["tag:app"]);
        assert_eq!(check_enrollment(Some(&placed), &asked), Ok(()));
    }

    #[test]
    fn the_order_of_the_tags_does_not_matter() {
        let placed = stack(Some("vaultwarden"), &["tag:a", "tag:b"], StackState::Active);
        let asked = request("vaultwarden", &["tag:b", "tag:a"]);
        assert_eq!(check_enrollment(Some(&placed), &asked), Ok(()));
    }

    #[test]
    fn a_stack_the_host_does_not_have_is_not_placed() {
        let asked = request("vaultwarden", &["tag:app"]);
        assert_eq!(check_enrollment(None, &asked), Err(Misplaced::NotPlaced));
    }

    #[test]
    fn a_stack_that_exposes_nothing_is_not_placed() {
        let placed = stack(None, &["tag:app"], StackState::Active);
        let asked = request("vaultwarden", &["tag:app"]);
        assert_eq!(
            check_enrollment(Some(&placed), &asked),
            Err(Misplaced::NotPlaced)
        );
    }

    #[test]
    fn a_hostname_other_than_the_exposed_service_is_not_placed() {
        let placed = stack(Some("vaultwarden"), &["tag:app"], StackState::Active);
        let asked = request("somebody-else", &["tag:app"]);
        assert_eq!(
            check_enrollment(Some(&placed), &asked),
            Err(Misplaced::NotPlaced)
        );
    }

    #[test]
    fn a_purged_stack_is_not_placed() {
        let placed = stack(Some("vaultwarden"), &["tag:app"], StackState::Purged);
        let asked = request("vaultwarden", &["tag:app"]);
        assert_eq!(
            check_enrollment(Some(&placed), &asked),
            Err(Misplaced::NotPlaced)
        );
    }

    #[test]
    fn an_extra_or_a_missing_tag_is_wrong() {
        let placed = stack(Some("vaultwarden"), &["tag:app"], StackState::Active);
        let extra = request("vaultwarden", &["tag:app", "tag:admin"]);
        let missing = request("vaultwarden", &[]);
        assert_eq!(
            check_enrollment(Some(&placed), &extra),
            Err(Misplaced::WrongTags)
        );
        assert_eq!(
            check_enrollment(Some(&placed), &missing),
            Err(Misplaced::WrongTags)
        );
    }

    #[test]
    fn only_a_purged_stack_owns_its_node_for_deletion() {
        let node = NodeLabel::new("vaultwarden").unwrap();
        let purged = stack(Some("vaultwarden"), &[], StackState::Purged);
        let active = stack(Some("vaultwarden"), &[], StackState::Active);
        let other = stack(Some("other"), &[], StackState::Purged);
        assert!(is_purged_node(&purged, &node));
        assert!(!is_purged_node(&active, &node));
        assert!(!is_purged_node(&other, &node));
    }
}
