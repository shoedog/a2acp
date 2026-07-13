use bridge_core::diagnostics::{
    AuthenticationEvidence, AuthenticationEvidenceInput, DiagnosticCode, DiagnosticEvent,
    DiagnosticFailureClass, DiagnosticOperation, DiagnosticPhase, DiagnosticRedactor,
    FailureDiagnostic, FailureDiagnosticInput, FailureDisposition, PersistedPhaseTransition,
    PersistedPhaseTransitionInput, PhaseStatus, StderrRedaction, StderrScope,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::OperationId;
use bridge_core::orch::{OrchEvent, OrchEventKind, ProgressPayload, ORCH_V};
use serde::{Deserialize, Serialize};

fn base_failure(code: &str) -> FailureDiagnosticInput {
    FailureDiagnosticInput {
        failed_phase: DiagnosticPhase::Initialize,
        last_completed_phase: Some(DiagnosticPhase::Spawn),
        class: DiagnosticFailureClass::Transport,
        disposition: FailureDisposition::Fatal,
        code: code.to_owned(),
        summary: "initialize failed".to_owned(),
        causes: Vec::new(),
        stderr_observed: false,
        stderr_line_count: 0,
        stderr_scope: None,
        stderr_tail: None,
        stderr_redaction: None,
        retry_after_ms: None,
        reset_at_ms: None,
        prompt_may_have_been_accepted: false,
    }
}

fn transition(redactor: &DiagnosticRedactor, status: PhaseStatus) -> PersistedPhaseTransition {
    PersistedPhaseTransition::build(
        PersistedPhaseTransitionInput {
            phase: DiagnosticPhase::Initialize,
            status,
            at_ms: 42,
            operation: Some(DiagnosticOperation::Model),
            code: Some("acp.initialize.transport".to_owned()),
            auth: None,
        },
        redactor,
    )
    .unwrap()
}

#[test]
fn diagnostic_code_accepts_only_bounded_bridge_tokens() {
    let redactor = DiagnosticRedactor::new(["known.secret"]);
    assert_eq!(
        DiagnosticCode::build("acp.initialize.timeout", &redactor)
            .unwrap()
            .as_str(),
        "acp.initialize.timeout"
    );

    for invalid in [
        "",
        "UPPERCASE",
        "has whitespace",
        "has/slash",
        "known.secret",
        "sk-proj-abcdefghijklmnopqrstuvwxyz012345",
    ] {
        assert!(
            DiagnosticCode::build(invalid, &redactor).is_err(),
            "accepted invalid diagnostic code {invalid:?}"
        );
    }
    assert!(DiagnosticCode::build("a".repeat(65), &redactor).is_err());
}

#[test]
fn static_codes_survive_short_secret_collision_without_weakening_dynamic_redaction() {
    let redactor = DiagnosticRedactor::new(["a"]);
    let transition = PersistedPhaseTransition::build_static_code(
        PersistedPhaseTransitionInput {
            phase: DiagnosticPhase::Authenticate,
            status: PhaseStatus::Skipped,
            at_ms: 42,
            operation: None,
            code: Some("a".into()),
            auth: Some(AuthenticationEvidenceInput::ConfiguredMethod {
                configured_id: "a".into(),
                advertised: false,
            }),
        },
        Some("acp.auth.no_methods_advertised"),
        &redactor,
    )
    .unwrap();
    assert_eq!(
        transition.code().map(|code| code.as_str()),
        Some("acp.auth.no_methods_advertised")
    );
    let transition_json = serde_json::to_string(&transition).unwrap();
    assert!(!transition_json.contains("\"value\":\"a\""));

    let mut input = base_failure("acp.initialize.transport");
    input.code = "a".into();
    input.causes = vec!["a".into()];
    let failure =
        FailureDiagnostic::build_static_code(input, "acp.initialize.transport", &redactor).unwrap();
    assert_eq!(failure.code().as_str(), "acp.initialize.transport");
    assert_eq!(failure.causes(), ["[REDACTED KNOWN SECRET]"]);
}

#[test]
fn cause_truncation_keeps_two_outermost_and_six_deepest() {
    let redactor = DiagnosticRedactor::default();
    let mut input = base_failure("acp.initialize.transport");
    input.causes = (0..10).map(|index| format!("cause-{index}")).collect();

    let diagnostic = FailureDiagnostic::build(input, &redactor).unwrap();
    assert_eq!(
        diagnostic.causes(),
        ["cause-0", "cause-1", "cause-4", "cause-5", "cause-6", "cause-7", "cause-8", "cause-9",]
    );
}

#[test]
fn redaction_covers_unsplit_adjacent_and_multibyte_values() {
    let secret = "sëcret-token";
    let redactor = DiagnosticRedactor::new([secret, "two-part"]).with_home_dir("/Users/operator");
    let mut input = base_failure("acp.initialize.transport");
    input.summary = format!("unsplit={secret}");
    input.causes = vec!["së".into(), "cret-".into(), "token".into()];
    input.stderr_observed = true;
    input.stderr_line_count = 2;
    input.stderr_scope = Some(StderrScope::Process);
    input.stderr_tail = Some(vec!["two-".into(), "part".into()]);
    input.stderr_redaction = Some(StderrRedaction::BestEffort);

    let diagnostic = FailureDiagnostic::build(input, &redactor).unwrap();
    let json = serde_json::to_string(&diagnostic).unwrap();
    assert!(!json.contains(secret));
    assert!(!json.contains("së\",\"cret-\",\"token"));
    assert_eq!(diagnostic.summary(), "unsplit=[REDACTED KNOWN SECRET]");
    assert!(diagnostic
        .causes()
        .iter()
        .all(|cause| cause == "[REDACTED KNOWN SECRET]"));
    assert!(diagnostic
        .stderr_tail()
        .unwrap()
        .iter()
        .all(|line| line == "[REDACTED KNOWN SECRET]"));

    let mut long = base_failure("acp.initialize.transport");
    long.summary = "é".repeat(300);
    let bounded = FailureDiagnostic::build(long, &DiagnosticRedactor::default()).unwrap();
    assert!(bounded.summary().len() <= 512);
    assert!(bounded.summary().is_char_boundary(bounded.summary().len()));
}

#[test]
fn redactor_extension_removes_runtime_derived_values() {
    let redactor = DiagnosticRedactor::new(["configured-template"]).with_known_values([
        "runtime-derived-secret",
        "runtime-derived-secret",
        "",
    ]);
    let mut input = base_failure("acp.initialize.transport");
    input.causes = vec!["agent echoed runtime-derived-secret".to_owned()];

    let diagnostic = FailureDiagnostic::build(input, &redactor).unwrap();
    let rendered = serde_json::to_string(&diagnostic).unwrap();
    assert!(!rendered.contains("runtime-derived-secret"));
    assert!(rendered.contains("REDACTED KNOWN SECRET"));
}

#[test]
fn redactor_removes_markers_urls_home_and_controls() {
    let redactor = DiagnosticRedactor::default().with_home_dir("/Users/operator");
    let mut input = base_failure("acp.initialize.transport");
    input.summary = "Authorization: Bearer TOPSECRET".into();
    input.causes = vec![
        "GET HTTPS://example.test/path?token=TOPSECRET#fragment".into(),
        "/Users/operator/private\u{0007}/file".into(),
    ];
    let diagnostic = FailureDiagnostic::build(input, &redactor).unwrap();
    let json = serde_json::to_string(&diagnostic).unwrap();
    assert!(!json.contains("TOPSECRET"));
    assert!(!json.contains("fragment"));
    assert!(!json.contains("/Users/operator"));
    assert!(!json.contains("\\u0007"));
}

#[test]
fn deserialization_resanitizes_mixed_case_url_secrets() {
    let diagnostic = FailureDiagnostic::build(
        base_failure("acp.initialize.transport"),
        &DiagnosticRedactor::default(),
    )
    .unwrap();
    let mut wire = serde_json::to_value(diagnostic).unwrap();
    wire["summary"] = serde_json::json!("GET HtTpS://example.test/path?api_key=TOPSECRET#fragment");

    let rebuilt: FailureDiagnostic = serde_json::from_value(wire).unwrap();
    let rendered = serde_json::to_string(&rebuilt).unwrap();
    assert!(!rendered.contains("TOPSECRET"));
    assert!(!rendered.contains("fragment"));
}

#[test]
fn authentication_ids_are_all_or_nothing_redacted() {
    let redactor = DiagnosticRedactor::new(["chat-gpt", "API_SECRET_NAME"]);

    let exact = AuthenticationEvidence::build(
        AuthenticationEvidenceInput::ConfiguredMethod {
            configured_id: "chat-gpt".into(),
            advertised: true,
        },
        &redactor,
    );
    let containing = AuthenticationEvidence::build(
        AuthenticationEvidenceInput::SelectedAdvertisedMethod {
            selected_id: "prefix-chat-gpt-suffix".into(),
        },
        &redactor,
    );
    let split = AuthenticationEvidence::build(
        AuthenticationEvidenceInput::PreAuthenticated {
            advertised_method_ids: vec!["chat-".into(), "g".into(), "pt".into()],
        },
        &redactor,
    );
    let env = AuthenticationEvidence::build(
        AuthenticationEvidenceInput::ApiKeyEnv {
            name: "API_SECRET_NAME".into(),
            present: true,
        },
        &redactor,
    );
    let safe = AuthenticationEvidence::build(
        AuthenticationEvidenceInput::SelectedAdvertisedMethod {
            selected_id: "oauth-device".into(),
        },
        &redactor,
    );

    for value in [&exact, &containing, &split, &env] {
        let json = serde_json::to_value(value).unwrap();
        let rendered = json.to_string();
        assert!(rendered.contains("\"state\":\"redacted\""));
        assert!(!rendered.contains("chat-gpt"));
        assert!(!rendered.contains("API_SECRET_NAME"));
    }
    assert_eq!(
        serde_json::to_value(&safe).unwrap()["selected_id"],
        serde_json::json!({"state":"value", "value":"oauth-device"})
    );
    let safe_json = serde_json::to_value(&safe).unwrap();
    let safe_roundtrip: AuthenticationEvidence = serde_json::from_value(safe_json).unwrap();
    assert_eq!(safe_roundtrip, safe);
}

#[test]
fn deserialization_cannot_reintroduce_an_unsanitized_id() {
    let unsafe_wire = serde_json::json!({
        "kind": "configured_method",
        "configured_id": {
            "state": "value",
            "value": "Authorization: unsafe-value"
        },
        "advertised": true
    });
    let evidence: AuthenticationEvidence = serde_json::from_value(unsafe_wire).unwrap();
    let rendered = serde_json::to_string(&evidence).unwrap();
    assert!(rendered.contains("\"state\":\"redacted\""));
    assert!(!rendered.contains("unsafe-value"));
}

#[test]
fn failure_error_formatting_and_wire_category_are_static() {
    let secret = "wire-secret-value";
    let redactor = DiagnosticRedactor::new([secret]);
    let mut input = base_failure("acp.initialize.transport");
    input.summary = format!("path /private/tmp/x contains {secret}");
    input.causes = vec![format!("SDK said {secret}")];
    let diagnostic = FailureDiagnostic::build(input, &redactor).unwrap();
    let error = BridgeError::agent_failure(diagnostic);

    assert_eq!(error.to_string(), "agent crashed");
    assert_eq!(error.client_message(), "agent crashed");
    assert!(!format!("{error:?}").contains(secret));
    assert!(!serde_json::to_string(&error.client_message())
        .unwrap()
        .contains(secret));
    assert!(!error.is_transient());
}

#[test]
fn transient_behavior_depends_only_on_typed_disposition() {
    let redactor = DiagnosticRedactor::default();
    for disposition in FailureDisposition::ALL {
        let expected = match disposition {
            FailureDisposition::Fatal => false,
            FailureDisposition::RetrySameTarget => true,
            FailureDisposition::ContainerFallbackCandidate => false,
        };
        let mut input = base_failure("acp.initialize.transport");
        input.disposition = disposition;
        if disposition == FailureDisposition::ContainerFallbackCandidate {
            input.class = DiagnosticFailureClass::ContainerRuntime;
        }
        let diagnostic = FailureDiagnostic::build(input, &redactor).unwrap();
        assert_eq!(
            BridgeError::agent_failure(diagnostic).is_transient(),
            expected
        );
    }

    let mut post_acceptance = base_failure("acp.prompt_stream.transport");
    post_acceptance.disposition = FailureDisposition::RetrySameTarget;
    post_acceptance.prompt_may_have_been_accepted = true;
    assert!(FailureDiagnostic::build(post_acceptance, &redactor).is_err());
}

#[test]
fn class_phase_and_barrier_constrain_nonfatal_dispositions() {
    let redactor = DiagnosticRedactor::default();
    for class in DiagnosticFailureClass::ALL {
        let mut retry = base_failure("diagnostic.retry.test");
        retry.class = class;
        retry.disposition = FailureDisposition::RetrySameTarget;
        let retry_allowed = matches!(
            class,
            DiagnosticFailureClass::Transport
                | DiagnosticFailureClass::AgentProcess
                | DiagnosticFailureClass::Timeout
                | DiagnosticFailureClass::Overloaded
        );
        assert_eq!(
            FailureDiagnostic::build(retry, &redactor).is_ok(),
            retry_allowed,
            "unexpected RetrySameTarget result for {class:?}"
        );

        let mut fallback = base_failure("diagnostic.fallback.test");
        fallback.class = class;
        fallback.disposition = FailureDisposition::ContainerFallbackCandidate;
        assert_eq!(
            FailureDiagnostic::build(fallback, &redactor).is_ok(),
            class.is_container_fallback_class(),
            "unexpected ContainerFallbackCandidate result for {class:?}"
        );
    }

    let mut post_barrier = base_failure("container.runtime.prompt_stream");
    post_barrier.failed_phase = DiagnosticPhase::PromptStream;
    post_barrier.class = DiagnosticFailureClass::ContainerRuntime;
    post_barrier.disposition = FailureDisposition::ContainerFallbackCandidate;
    assert!(FailureDiagnostic::build(post_barrier, &redactor).is_err());

    let mut valid_post_barrier = base_failure("container.runtime.prompt_stream");
    valid_post_barrier.failed_phase = DiagnosticPhase::PromptStream;
    valid_post_barrier.class = DiagnosticFailureClass::ContainerRuntime;
    valid_post_barrier.disposition = FailureDisposition::Fatal;
    valid_post_barrier.prompt_may_have_been_accepted = true;
    let diagnostic = FailureDiagnostic::build(valid_post_barrier, &redactor).unwrap();
    let mut wire = serde_json::to_value(diagnostic).unwrap();
    wire["disposition"] = serde_json::json!("container_fallback_candidate");
    wire["prompt_may_have_been_accepted"] = serde_json::json!(false);
    assert!(serde_json::from_value::<FailureDiagnostic>(wire).is_err());
}

#[test]
fn every_diagnostic_class_has_a_bounded_metrics_mapping() {
    use bridge_core::ports::{classify_failure, FailureClass};

    let cases = [
        (DiagnosticFailureClass::Config, FailureClass::Config),
        (DiagnosticFailureClass::Authentication, FailureClass::Config),
        (DiagnosticFailureClass::Model, FailureClass::Config),
        (DiagnosticFailureClass::Protocol, FailureClass::Transport),
        (DiagnosticFailureClass::Transport, FailureClass::Transport),
        (
            DiagnosticFailureClass::AgentProcess,
            FailureClass::AgentCrashed,
        ),
        (
            DiagnosticFailureClass::ContainerRuntime,
            FailureClass::AgentCrashed,
        ),
        (
            DiagnosticFailureClass::ContainerImage,
            FailureClass::AgentCrashed,
        ),
        (
            DiagnosticFailureClass::ContainerNetwork,
            FailureClass::AgentCrashed,
        ),
        (
            DiagnosticFailureClass::ContainerMount,
            FailureClass::AgentCrashed,
        ),
        (
            DiagnosticFailureClass::ContainerCredentials,
            FailureClass::AgentCrashed,
        ),
        (DiagnosticFailureClass::Timeout, FailureClass::TimedOut),
        (DiagnosticFailureClass::Overloaded, FailureClass::Overloaded),
        (
            DiagnosticFailureClass::ProviderLimit,
            FailureClass::Overloaded,
        ),
        (DiagnosticFailureClass::Persistence, FailureClass::Other),
        (DiagnosticFailureClass::Canceled, FailureClass::Other),
        (DiagnosticFailureClass::Unknown, FailureClass::Other),
    ];
    assert_eq!(cases.len(), DiagnosticFailureClass::ALL.len());
    for (class, expected) in cases {
        let mut input = base_failure("diagnostic.test.failure");
        input.class = class;
        let diagnostic = FailureDiagnostic::build(input, &DiagnosticRedactor::default()).unwrap();
        assert_eq!(
            classify_failure(&BridgeError::agent_failure(diagnostic)),
            expected
        );
    }
}

#[test]
fn production_sources_construct_agent_failure_only_through_central_builder() {
    use syn::visit::Visit;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum CfgValue {
        True,
        False,
        Unknown,
    }

    fn evaluate_cfg(meta: &syn::Meta) -> CfgValue {
        match meta {
            syn::Meta::Path(path) if path.is_ident("test") => CfgValue::False,
            syn::Meta::Path(_) | syn::Meta::NameValue(_) => CfgValue::Unknown,
            syn::Meta::List(list) => {
                let nested = list
                    .parse_args_with(
                        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                    )
                    .unwrap_or_default();
                if list.path.is_ident("not") {
                    return match nested.first().map(evaluate_cfg) {
                        Some(CfgValue::True) => CfgValue::False,
                        Some(CfgValue::False) => CfgValue::True,
                        Some(CfgValue::Unknown) | None => CfgValue::Unknown,
                    };
                }
                if list.path.is_ident("all") {
                    if nested
                        .iter()
                        .any(|meta| evaluate_cfg(meta) == CfgValue::False)
                    {
                        return CfgValue::False;
                    }
                    if nested
                        .iter()
                        .all(|meta| evaluate_cfg(meta) == CfgValue::True)
                    {
                        return CfgValue::True;
                    }
                    return CfgValue::Unknown;
                }
                if list.path.is_ident("any") {
                    if nested
                        .iter()
                        .any(|meta| evaluate_cfg(meta) == CfgValue::True)
                    {
                        return CfgValue::True;
                    }
                    if nested
                        .iter()
                        .all(|meta| evaluate_cfg(meta) == CfgValue::False)
                    {
                        return CfgValue::False;
                    }
                }
                CfgValue::Unknown
            }
        }
    }

    fn test_only(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|attr| {
            if attr.path().is_ident("test") {
                return true;
            }
            if !attr.path().is_ident("cfg") {
                return false;
            }
            attr.meta
                .require_list()
                .ok()
                .and_then(|list| syn::parse2::<syn::Meta>(list.tokens.clone()).ok())
                .is_some_and(|meta| evaluate_cfg(&meta) == CfgValue::False)
        })
    }

    fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
        match item {
            syn::Item::Const(item) => &item.attrs,
            syn::Item::Enum(item) => &item.attrs,
            syn::Item::ExternCrate(item) => &item.attrs,
            syn::Item::Fn(item) => &item.attrs,
            syn::Item::ForeignMod(item) => &item.attrs,
            syn::Item::Impl(item) => &item.attrs,
            syn::Item::Macro(item) => &item.attrs,
            syn::Item::Mod(item) => &item.attrs,
            syn::Item::Static(item) => &item.attrs,
            syn::Item::Struct(item) => &item.attrs,
            syn::Item::Trait(item) => &item.attrs,
            syn::Item::TraitAlias(item) => &item.attrs,
            syn::Item::Type(item) => &item.attrs,
            syn::Item::Union(item) => &item.attrs,
            syn::Item::Use(item) => &item.attrs,
            syn::Item::Verbatim(_) => &[],
            _ => &[],
        }
    }

    fn forbidden_use(tree: &syn::UseTree) -> bool {
        match tree {
            syn::UseTree::Path(path) => forbidden_use(&path.tree),
            syn::UseTree::Name(name) => {
                name.ident == "AgentFailure" || name.ident == "agent_failure"
            }
            syn::UseTree::Rename(rename) => {
                rename.ident == "AgentFailure" || rename.ident == "agent_failure"
            }
            syn::UseTree::Group(group) => group.items.iter().any(forbidden_use),
            syn::UseTree::Glob(_) => false,
        }
    }

    struct ConstructorVisitor<'a> {
        path: &'a std::path::Path,
        violations: Vec<String>,
        in_central_builder: bool,
        central_builder_constructor_count: usize,
        in_acp_lifecycle_impl: bool,
        in_acp_lifecycle_failure: bool,
        acp_lifecycle_constructor_count: usize,
    }

    impl<'ast> Visit<'ast> for ConstructorVisitor<'_> {
        fn visit_item(&mut self, node: &'ast syn::Item) {
            if !test_only(item_attrs(node)) {
                syn::visit::visit_item(self, node);
            }
        }

        fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
            if forbidden_use(&node.tree) {
                self.violations.push(format!(
                    "{}: forbidden AgentFailure import or rename",
                    self.path.display()
                ));
            }
            syn::visit::visit_item_use(self, node);
        }

        fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
            let was_in_acp_lifecycle_impl = self.in_acp_lifecycle_impl;
            self.in_acp_lifecycle_impl =
                self.path.ends_with("crates/bridge-acp/src/acp_backend.rs")
                    && node.trait_.is_none()
                    && matches!(
                        node.self_ty.as_ref(),
                        syn::Type::Path(path)
                            if path.qself.is_none()
                                && path.path.segments.len() == 1
                                && path.path.is_ident("AcpLifecycle")
                    );
            syn::visit::visit_item_impl(self, node);
            self.in_acp_lifecycle_impl = was_in_acp_lifecycle_impl;
        }

        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            if test_only(&node.attrs) {
                return;
            }
            let was_in_central_builder = self.in_central_builder;
            if self.path.ends_with("crates/bridge-core/src/error.rs")
                && node.sig.ident == "agent_failure"
            {
                self.in_central_builder = true;
            }
            let was_in_acp_lifecycle_failure = self.in_acp_lifecycle_failure;
            self.in_acp_lifecycle_failure =
                self.in_acp_lifecycle_impl && node.sig.ident == "failure";
            syn::visit::visit_impl_item_fn(self, node);
            self.in_central_builder = was_in_central_builder;
            self.in_acp_lifecycle_failure = was_in_acp_lifecycle_failure;
        }

        fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
            if node
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "AgentFailure")
            {
                if self.in_central_builder {
                    self.central_builder_constructor_count += 1;
                } else {
                    self.violations.push(format!(
                        "{}: AgentFailure struct expression",
                        self.path.display()
                    ));
                }
            }
            syn::visit::visit_expr_struct(self, node);
        }

        fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
            if node
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "agent_failure")
            {
                self.violations.push(format!(
                    "{}: agent_failure constructor reference",
                    self.path.display()
                ));
            }
            syn::visit::visit_expr_path(self, node);
        }

        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            let is_agent_failure_call = match node.func.as_ref() {
                syn::Expr::Path(path) => {
                    let segments: Vec<_> = path.path.segments.iter().collect();
                    segments.len() >= 2
                        && segments[segments.len() - 2].ident == "BridgeError"
                        && segments[segments.len() - 1].ident == "agent_failure"
                }
                _ => false,
            };
            if is_agent_failure_call {
                let exact_path = matches!(node.func.as_ref(), syn::Expr::Path(path)
                    if path.qself.is_none() && path.path.segments.len() == 2);
                if self.in_acp_lifecycle_failure && exact_path {
                    self.acp_lifecycle_constructor_count += 1;
                } else {
                    self.violations.push(format!(
                        "{}: BridgeError::agent_failure outside AcpLifecycle::failure",
                        self.path.display()
                    ));
                }
                for argument in &node.args {
                    self.visit_expr(argument);
                }
            } else {
                syn::visit::visit_expr_call(self, node);
            }
        }

        fn visit_macro(&mut self, node: &'ast syn::Macro) {
            let tokens = node.tokens.to_string();
            if tokens.contains("AgentFailure") || tokens.contains("agent_failure") {
                self.violations.push(format!(
                    "{}: AgentFailure token in production macro",
                    self.path.display()
                ));
            }
            syn::visit::visit_macro(self, node);
        }
    }

    fn source_violations_at(path: &std::path::Path, source: &str) -> Vec<String> {
        let file = syn::parse_file(source).unwrap();
        let mut visitor = ConstructorVisitor {
            path,
            violations: Vec::new(),
            in_central_builder: false,
            central_builder_constructor_count: 0,
            in_acp_lifecycle_impl: false,
            in_acp_lifecycle_failure: false,
            acp_lifecycle_constructor_count: 0,
        };
        visitor.visit_file(&file);
        if path.ends_with("crates/bridge-core/src/error.rs")
            && visitor.central_builder_constructor_count != 1
        {
            visitor.violations.push(format!(
                "{}: central builder contains {} AgentFailure constructors, expected 1",
                path.display(),
                visitor.central_builder_constructor_count
            ));
        }
        if path.ends_with("crates/bridge-acp/src/acp_backend.rs")
            && visitor.acp_lifecycle_constructor_count != 1
        {
            visitor.violations.push(format!(
                "{}: AcpLifecycle::failure contains {} agent_failure calls, expected 1",
                path.display(),
                visitor.acp_lifecycle_constructor_count
            ));
        }
        visitor.violations
    }

    fn source_violations(source: &str) -> Vec<String> {
        source_violations_at(std::path::Path::new("synthetic.rs"), source)
    }

    fn visit(path: &std::path::Path, violations: &mut Vec<String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, violations);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs")
                && path
                    .components()
                    .any(|component| component.as_os_str() == "src")
            {
                let source = std::fs::read_to_string(&path).unwrap();
                syn::parse_file(&source).unwrap_or_else(|error| {
                    panic!(
                        "failed to parse {} for source guard: {error}",
                        path.display()
                    )
                });
                violations.extend(source_violations_at(&path, &source));
            }
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut violations = Vec::new();
    visit(&root.join("crates"), &mut violations);
    visit(&root.join("bin"), &mut violations);
    assert!(
        violations.is_empty(),
        "R2b1 forbids production AgentFailure construction: {violations:?}"
    );

    assert!(source_violations(
        "#[cfg(test)] fn fixture() { BridgeError::agent_failure(diagnostic); }"
    )
    .is_empty());
    assert!(source_violations_at(
        std::path::Path::new("crates/bridge-acp/src/acp_backend.rs"),
        "struct AcpLifecycle; impl AcpLifecycle { \
         async fn failure(&self) { let _ = BridgeError::agent_failure(diagnostic); } }"
    )
    .is_empty());
    assert!(
        !source_violations("fn production_site() { BridgeError::agent_failure(diagnostic); }")
            .is_empty(),
        "an arbitrary production site must not bypass the central lifecycle builder"
    );
    for source in [
        "use BridgeError::AgentFailure as AF; fn f() { let _ = AF { diagnostic }; }",
        "use BridgeError::agent_failure as make; fn f() { let _ = make(diagnostic); }",
        "fn f() { let make = BridgeError::agent_failure; make(diagnostic); }",
        "fn f() { Other::agent_failure(diagnostic); }",
    ] {
        assert!(
            !source_violations(source).is_empty(),
            "source guard missed production-capable constructor: {source}"
        );
    }
}

#[derive(Debug, Deserialize)]
struct PriorOrchEvent {
    #[allow(dead_code)]
    v: u16,
    #[allow(dead_code)]
    seq: i64,
    #[allow(dead_code)]
    ts_ms: i64,
    #[allow(dead_code)]
    operation_id: OperationId,
    #[serde(flatten)]
    kind: PriorOrchEventKind,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PriorOrchEventKind {
    Progress { text: String },
}

#[test]
fn progress_payload_is_backward_and_rollback_compatible() {
    let old_json = serde_json::json!({
        "v": ORCH_V,
        "seq": 1,
        "ts_ms": 42,
        "operation_id": "op-r2b1",
        "kind": "progress",
        "text": "legacy"
    });
    let read_by_new: OrchEvent = serde_json::from_value(old_json).unwrap();
    assert!(matches!(
        read_by_new.kind,
        OrchEventKind::Progress { ref progress }
            if progress.text() == "legacy" && progress.diagnostic_event().is_none()
    ));
    let legacy_new_writer = serde_json::to_value(OrchEventKind::Progress {
        progress: ProgressPayload::legacy("legacy"),
    })
    .unwrap();
    assert_eq!(
        legacy_new_writer,
        serde_json::json!({"kind": "progress", "text": "legacy"})
    );

    let diagnostic = DiagnosticEvent::new(
        transition(&DiagnosticRedactor::default(), PhaseStatus::Started),
        None,
    )
    .unwrap();
    let new_event = OrchEvent {
        v: ORCH_V,
        seq: 2,
        ts_ms: 43,
        operation_id: OperationId::parse("op-r2b1").unwrap(),
        session: None,
        source: None,
        kind: OrchEventKind::Progress {
            progress: ProgressPayload::diagnostic(diagnostic),
        },
    };
    let json = serde_json::to_value(&new_event).unwrap();
    assert_eq!(json["kind"], "progress");
    assert_eq!(json["text"], "diagnostic transition");
    assert!(json.get("diagnostic").is_some());

    let mut tampered = json.clone();
    tampered["text"] = serde_json::json!("Bearer KNOWN_SECRET");
    assert!(serde_json::from_value::<OrchEvent>(tampered).is_err());

    let read_by_prior: PriorOrchEvent = serde_json::from_value(json).unwrap();
    assert!(matches!(
        read_by_prior.kind,
        PriorOrchEventKind::Progress { ref text } if text == "diagnostic transition"
    ));
}

#[test]
fn diagnostic_vocabulary_serializes_as_closed_snake_case_values() {
    assert_eq!(
        serde_json::to_string(&DiagnosticPhase::PromptStream).unwrap(),
        "\"prompt_stream\""
    );
    assert_eq!(
        serde_json::to_string(&DiagnosticFailureClass::ContainerCredentials).unwrap(),
        "\"container_credentials\""
    );
    assert_eq!(
        serde_json::to_string(&FailureDisposition::RetrySameTarget).unwrap(),
        "\"retry_same_target\""
    );
}

#[test]
fn opted_in_stderr_requires_typed_scope_and_redaction() {
    let mut zero = base_failure("acp.initialize.transport");
    zero.stderr_observed = true;
    zero.stderr_scope = Some(StderrScope::Process);
    assert!(FailureDiagnostic::build(zero, &DiagnosticRedactor::default()).is_err());

    let mut input = base_failure("acp.initialize.transport");
    input.stderr_observed = true;
    input.stderr_line_count = 1;
    input.stderr_tail = Some(vec!["safe opaque line".into()]);
    assert!(FailureDiagnostic::build(input.clone(), &DiagnosticRedactor::default()).is_err());

    input.stderr_scope = Some(StderrScope::Process);
    input.stderr_redaction = Some(StderrRedaction::BestEffort);
    assert!(FailureDiagnostic::build(input.clone(), &DiagnosticRedactor::default()).is_ok());

    input.stderr_tail = Some(vec!["one".into(), "two".into()]);
    assert!(FailureDiagnostic::build(input, &DiagnosticRedactor::default()).is_err());

    let mut capped = base_failure("acp.initialize.transport");
    capped.stderr_observed = true;
    capped.stderr_line_count = 33;
    capped.stderr_scope = Some(StderrScope::Process);
    capped.stderr_tail = Some((0..33).map(|index| format!("line-{index}")).collect());
    capped.stderr_redaction = Some(StderrRedaction::BestEffort);
    let capped = FailureDiagnostic::build(capped, &DiagnosticRedactor::default()).unwrap();
    assert_eq!(capped.stderr_tail().unwrap().len(), 32);
}

#[test]
fn reset_timestamp_is_bounded_to_thirty_days_from_reference_time() {
    const NOW_MS: i64 = 1_000_000;
    const THIRTY_DAYS_MS: i64 = 2_592_000_000;

    let mut boundary = base_failure("upstream.provider_limit");
    boundary.reset_at_ms = Some(NOW_MS + THIRTY_DAYS_MS);
    assert!(
        FailureDiagnostic::build_at(boundary.clone(), &DiagnosticRedactor::default(), NOW_MS)
            .is_ok()
    );

    boundary.reset_at_ms = Some(NOW_MS + THIRTY_DAYS_MS + 1);
    assert!(FailureDiagnostic::build_at(boundary, &DiagnosticRedactor::default(), NOW_MS).is_err());

    let mut extreme = base_failure("upstream.provider_limit");
    extreme.reset_at_ms = Some(i64::MAX);
    assert!(FailureDiagnostic::build_at(extreme, &DiagnosticRedactor::default(), NOW_MS).is_err());

    let mut missing_clock = base_failure("upstream.provider_limit");
    missing_clock.reset_at_ms = Some(NOW_MS);
    assert!(FailureDiagnostic::build_with_reference_time(
        missing_clock,
        &DiagnosticRedactor::default(),
        None
    )
    .is_err());
    assert!(FailureDiagnostic::build_with_reference_time(
        base_failure("acp.initialize.transport"),
        &DiagnosticRedactor::default(),
        None
    )
    .is_ok());

    for invalid_reference in [-1, i64::MAX] {
        let mut reset_bearing = base_failure("upstream.provider_limit");
        reset_bearing.reset_at_ms = Some(NOW_MS);
        assert!(FailureDiagnostic::build_with_reference_time(
            reset_bearing,
            &DiagnosticRedactor::default(),
            Some(invalid_reference)
        )
        .is_err());
        assert!(FailureDiagnostic::build_with_reference_time(
            base_failure("acp.initialize.transport"),
            &DiagnosticRedactor::default(),
            Some(invalid_reference)
        )
        .is_ok());
    }

    let current_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let diagnostic = FailureDiagnostic::build(
        base_failure("upstream.provider_limit"),
        &DiagnosticRedactor::default(),
    )
    .unwrap();
    let mut wire = serde_json::to_value(diagnostic).unwrap();
    wire["reset_at_ms"] = serde_json::json!(current_ms + THIRTY_DAYS_MS + 60_000);
    assert!(serde_json::from_value::<FailureDiagnostic>(wire).is_err());
}

#[test]
fn diagnostic_event_rejects_failure_on_non_failed_transition() {
    let redactor = DiagnosticRedactor::default();
    let failure =
        FailureDiagnostic::build(base_failure("acp.initialize.transport"), &redactor).unwrap();
    assert!(
        DiagnosticEvent::new(transition(&redactor, PhaseStatus::Completed), Some(failure)).is_err()
    );
}

// Keep the imports honest: diagnostic DTOs are serializable evidence, while inputs are not.
fn _assert_serializable<T: Serialize>() {}

#[test]
fn diagnostic_dtos_are_serializable() {
    _assert_serializable::<FailureDiagnostic>();
    _assert_serializable::<PersistedPhaseTransition>();
    _assert_serializable::<DiagnosticEvent>();
}
