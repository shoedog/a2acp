use bridge_core::diagnostics::{
    DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
    FailureDiagnosticInput, FailureDisposition,
};
use bridge_core::error::{warm_session_survivability, BridgeError, WarmSessionSurvivability};

fn structured(class: DiagnosticFailureClass, disposition: FailureDisposition) -> BridgeError {
    let pre_prompt = disposition != FailureDisposition::Fatal;
    let diagnostic = FailureDiagnostic::build_static_code(
        FailureDiagnosticInput {
            failed_phase: if pre_prompt {
                DiagnosticPhase::Resolve
            } else {
                DiagnosticPhase::PromptStart
            },
            last_completed_phase: None,
            class,
            disposition,
            code: "ignored".to_owned(),
            summary: "bounded test failure".to_owned(),
            causes: Vec::new(),
            stderr_observed: false,
            stderr_line_count: 0,
            stderr_scope: None,
            stderr_tail: None,
            stderr_redaction: None,
            retry_after_ms: None,
            reset_at_ms: None,
            prompt_may_have_been_accepted: !pre_prompt,
        },
        "test.warm.failure",
        &DiagnosticRedactor::default(),
    )
    .unwrap();
    BridgeError::agent_failure(diagnostic)
}

#[test]
fn every_structured_failure_class_expires_a_warm_session() {
    for class in DiagnosticFailureClass::ALL {
        let error = structured(class, FailureDisposition::Fatal);
        assert_eq!(
            warm_session_survivability(&error),
            WarmSessionSurvivability::Expire,
            "structured class {class:?} must fail closed"
        );
    }
}

#[test]
fn every_valid_nonfatal_disposition_still_expires_the_current_session() {
    for class in [
        DiagnosticFailureClass::Transport,
        DiagnosticFailureClass::AgentProcess,
        DiagnosticFailureClass::Timeout,
        DiagnosticFailureClass::Overloaded,
    ] {
        assert_eq!(
            warm_session_survivability(&structured(class, FailureDisposition::RetrySameTarget,)),
            WarmSessionSurvivability::Expire,
        );
    }
    for class in [
        DiagnosticFailureClass::ContainerRuntime,
        DiagnosticFailureClass::ContainerImage,
        DiagnosticFailureClass::ContainerNetwork,
        DiagnosticFailureClass::ContainerMount,
        DiagnosticFailureClass::ContainerCredentials,
    ] {
        assert_eq!(
            warm_session_survivability(&structured(
                class,
                FailureDisposition::ContainerFallbackCandidate,
            )),
            WarmSessionSurvivability::Expire,
        );
    }
}

#[test]
fn legacy_errors_preserve_the_owning_paths_existing_policy() {
    let legacy = [
        BridgeError::A2aVersionMismatch,
        BridgeError::InvalidRequest { field: "input" },
        BridgeError::TaskNotFound,
        BridgeError::SessionNotFound,
        BridgeError::ConfigMismatch { field: "model" },
        BridgeError::ConfigReseedRequired { field: "mode" },
        BridgeError::SessionExpired,
        BridgeError::HandleBusy,
        BridgeError::AuthRequired {
            request_id: "r".to_owned(),
        },
        BridgeError::PermissionRequired {
            request_id: "r".to_owned(),
        },
        BridgeError::PermissionDenied,
        BridgeError::AgentNotAuthenticated,
        BridgeError::ModelNotAvailable,
        BridgeError::CancelTimeout,
        BridgeError::AgentTimedOut,
        BridgeError::FrameError,
        BridgeError::MessageTooLarge,
        BridgeError::agent_crashed("legacy"),
        BridgeError::AgentOverloaded,
        BridgeError::UpstreamA2aError,
        BridgeError::StoreFailure,
        BridgeError::InvalidStateTransition,
        BridgeError::UnknownAgent { id: "a".to_owned() },
        BridgeError::config_invalid("legacy"),
        BridgeError::TaskSpecInvalid {
            message: "legacy".to_owned(),
        },
    ];

    for error in legacy {
        assert_eq!(
            warm_session_survivability(&error),
            WarmSessionSurvivability::PreserveOwnerBehavior,
            "legacy error changed owner policy: {error:?}"
        );
    }
}
