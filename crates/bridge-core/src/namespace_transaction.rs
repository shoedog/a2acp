mod mechanism {
    use crate::fs_custody::{
        capture_target_no_replace_v2, child_entry_exists_impl, child_name_cstring,
        create_new_regular_child_at, enumerate_directory_names, open_regular_child,
        rename_child_no_replace, required_file_content_snapshot_v2, ChildNameV2,
        CustodyCaptureOutcomeV2, CustodyIntentV2, CustodyOperationKindV2, FileContentSnapshotV2,
        FsCustodyError, JournalMutationOutcomeV2, JournalRootOperationV2, RenameNoReplaceRefusalV1,
        RequiredObjectIdentityV2, ReservedNameNamespaceV2,
    };
    use serde::{Deserialize, Serialize};
    use std::{
        fs::File,
        io::{Read, Write},
        os::{
            fd::AsRawFd as _,
            unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
        },
    };
    const CHILD_CAP: usize = 4096;
    type CommitmentV2 = [u8; 32];
    const INTENT_CAP: u64 = 4096;
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct NamespaceRecoveryTicketV2 {
        operation: CustodyOperationKindV2,
        target: ChildNameV2,
    }
    impl NamespaceRecoveryTicketV2 {
        #[must_use]
        pub fn operation(&self) -> CustodyOperationKindV2 {
            self.operation
        }
        #[must_use]
        pub fn target(&self) -> &ChildNameV2 {
            &self.target
        }
    }
    #[must_use]
    #[derive(Debug, PartialEq, Eq)]
    pub enum NamespaceTransactionOutcomeV2 {
        Ready,
        Complete(NamespaceRecoveryTicketV2),
        NoEffect(NamespaceRecoveryTicketV2, String),
        Retained(NamespaceRecoveryTicketV2, String),
        ProtectiveDebt(String),
        Unsupported(String),
    }
    pub struct NamespaceTransactionV2;
    struct SettledV2;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TransitionV2 {
        StageCreated,
        IntentSynced,
        BeforeCapture,
        Captured,
        Published,
        TargetSynced,
        Retired,
        ZeroLinkProved,
        IntentRemoved,
        FinalSync,
        BeforeCleanup,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct IntentWireV2 {
        schema: u8,
        operation: CustodyOperationKindV2,
        target: String,
        expected: RequiredObjectIdentityV2,
        staged: FileContentSnapshotV2,
        staged_sha256: Option<CommitmentV2>,
        reserved: [String; 4],
    }
    struct Failure(bool, String);
    impl From<String> for Failure {
        fn from(reason: String) -> Self {
            Self(false, reason)
        }
    }
    impl From<FsCustodyError> for Failure {
        fn from(error: FsCustodyError) -> Self {
            match error {
                FsCustodyError::Unsupported(reason) => Self(true, reason),
                error => Self(false, error.to_string()),
            }
        }
    }
    #[cfg(test)]
    thread_local! {
        static SNAPSHOT_UNSUPPORTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static CAPTURE_UNSUPPORTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    fn ticket(
        operation: CustodyOperationKindV2,
        target: &ChildNameV2,
    ) -> NamespaceRecoveryTicketV2 {
        NamespaceRecoveryTicketV2 {
            operation,
            target: target.clone(),
        }
    }
    fn debt(
        ticket: NamespaceRecoveryTicketV2,
        reason: impl Into<Failure>,
    ) -> NamespaceTransactionOutcomeV2 {
        let Failure(unsupported, reason) = reason.into();
        if unsupported {
            NamespaceTransactionOutcomeV2::Unsupported(reason)
        } else {
            NamespaceTransactionOutcomeV2::Retained(ticket, reason)
        }
    }
    fn protect(reason: impl Into<Failure>) -> NamespaceTransactionOutcomeV2 {
        let Failure(unsupported, reason) = reason.into();
        if unsupported {
            NamespaceTransactionOutcomeV2::Unsupported(reason)
        } else {
            NamespaceTransactionOutcomeV2::ProtectiveDebt(reason)
        }
    }
    fn record(
        op: &JournalRootOperationV2<'_>,
        outcome: NamespaceTransactionOutcomeV2,
    ) -> NamespaceTransactionOutcomeV2 {
        match outcome {
            value @ NamespaceTransactionOutcomeV2::Retained(_, _)
            | value @ NamespaceTransactionOutcomeV2::ProtectiveDebt(_) => {
                op.record_protective_debt(value)
            }
            value => value,
        }
    }
    fn capture(root: &File, intent: &CustodyIntentV2, label: &str) -> CustodyCaptureOutcomeV2 {
        #[cfg(test)]
        if CAPTURE_UNSUPPORTED.with(|fault| fault.replace(false)) {
            return CustodyCaptureOutcomeV2::RuntimeUnsupported(label.into());
        }
        capture_target_no_replace_v2(root, intent, label)
    }
    fn sync_root(root: &File, label: &str) -> Result<SettledV2, String> {
        root.sync_all()
            .map(|_| SettledV2)
            .map_err(|e| format!("{label}: root sync failed: {e}"))
    }
    fn snapshot(
        root: &File,
        name: &ChildNameV2,
        label: &str,
    ) -> Result<Option<(File, FileContentSnapshotV2)>, Failure> {
        #[cfg(test)]
        if SNAPSHOT_UNSUPPORTED.with(|fault| fault.replace(false)) {
            return Err(FsCustodyError::Unsupported(label.into()).into());
        }
        if !child_entry_exists_impl(root, name.as_os_str(), label)? {
            return Ok(None);
        }
        let file = open_regular_child(root, name.as_os_str(), label)?;
        let value = required_file_content_snapshot_v2(&file, label)?;
        Ok(Some((file, value)))
    }
    fn move_new(
        op: &JournalRootOperationV2<'_>,
        from: &ChildNameV2,
        to: &ChildNameV2,
        label: &str,
    ) -> Result<SettledV2, Failure> {
        op.prove_route_v2(label)?;
        let root = op.root_file();
        let from = child_name_cstring(from.as_os_str(), label)?;
        let to = child_name_cstring(to.as_os_str(), label)?;
        match rename_child_no_replace(root, &from, &to) {
            Ok(()) => {}
            Err(RenameNoReplaceRefusalV1::PlatformUnsupported) => {
                return Err(FsCustodyError::Unsupported(label.into()).into())
            }
            Err(RenameNoReplaceRefusalV1::Io(error))
                if [libc::ENOSYS, libc::ENOTSUP, libc::EOPNOTSUPP]
                    .contains(&error.raw_os_error().unwrap_or_default()) =>
            {
                return Err(FsCustodyError::Unsupported(label.into()).into());
            }
            Err(error) => {
                return Err(format!("{label}: no-replace rename refused: {error:?}").into())
            }
        }
        Ok(sync_root(root, label)?)
    }
    fn remove<F>(
        op: &JournalRootOperationV2<'_>,
        name: &ChildNameV2,
        expected: FileContentSnapshotV2,
        events: Option<(TransitionV2, TransitionV2)>,
        hook: &mut F,
        label: &str,
    ) -> Result<File, Failure>
    where
        F: FnMut(TransitionV2, &ChildNameV2) -> bool,
    {
        let root = op.root_file();
        let (held, before) =
            snapshot(root, name, label)?.ok_or_else(|| format!("{label}: unlink source absent"))?;
        if before != expected {
            return Err(format!("{label}: unlink identity/content changed").into());
        }
        if hook(TransitionV2::BeforeCleanup, name) {
            return Err(format!("{label}: interrupted before unlink").into());
        }
        let (_, current) = snapshot(root, name, label)?
            .ok_or_else(|| format!("{label}: unlink source disappeared"))?;
        if current != expected {
            return Err(format!("{label}: unlink source substituted").into());
        }
        op.prove_route_v2(label)?;
        let encoded = child_name_cstring(name.as_os_str(), label)?;
        if unsafe { libc::unlinkat(root.as_raw_fd(), encoded.as_ptr(), 0) } == -1 {
            return Err(format!(
                "{label}: exact unlink refused: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        if events.is_some_and(|e| hook(e.0, name)) {
            return Err(format!("{label}: interrupted after unlink").into());
        }
        if held.metadata().map_err(|e| e.to_string())?.nlink() != 0 {
            return Err(format!("{label}: retained fd still linked").into());
        }
        if events.is_some_and(|e| hook(e.1, name)) {
            return Err(format!("{label}: interrupted after zero-link proof").into());
        }
        sync_root(root, label)?;
        Ok(held)
    }
    fn to_wire(intent: &CustodyIntentV2, staged_sha256: Option<CommitmentV2>) -> IntentWireV2 {
        let (operation, target, expected, staged) = intent.parts();
        let reserved = ReservedNameNamespaceV2::ALL.map(|n| {
            intent
                .reserved_name(n)
                .as_os_str()
                .to_str()
                .expect("V2 name")
                .into()
        });
        IntentWireV2 {
            schema: 2,
            operation,
            target: target.as_os_str().to_str().expect("V2 name").into(),
            expected: *expected,
            staged: *staged,
            staged_sha256,
            reserved,
        }
    }
    fn decode(bytes: &[u8]) -> Result<(CustodyIntentV2, Option<CommitmentV2>), Failure> {
        let wire: IntentWireV2 =
            serde_json::from_slice(bytes).map_err(|e| format!("malformed intent: {e}"))?;
        if wire.schema != 2 {
            return Err("foreign intent schema".to_owned().into());
        }
        if wire.staged_sha256.is_some() != matches!(wire.operation, CustodyOperationKindV2::Replace)
        {
            return Err("invalid staged commitment".to_owned().into());
        }
        let target = ChildNameV2::from_bytes(wire.target.as_bytes())
            .map_err(|_| "invalid intent target".to_owned())?;
        let intent = CustodyIntentV2::new(wire.operation, target, wire.expected, wire.staged)?;
        if to_wire(&intent, wire.staged_sha256).reserved != wire.reserved {
            return Err("non-deterministic reserved names".to_owned().into());
        }
        Ok((intent, wire.staged_sha256))
    }
    fn create_intent(
        op: &JournalRootOperationV2<'_>,
        intent: &CustodyIntentV2,
        staged_sha256: Option<CommitmentV2>,
        label: &str,
    ) -> Result<FileContentSnapshotV2, Failure> {
        op.prove_route_v2(label)?;
        let root = op.root_file();
        let name = intent.reserved_name(ReservedNameNamespaceV2::TransactionIntent);
        let mut file = create_new_regular_child_at(root, name.as_os_str(), label)?;
        let bytes =
            serde_json::to_vec(&to_wire(intent, staged_sha256)).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > INTENT_CAP {
            return Err("intent exceeds cap".to_owned().into());
        }
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("{label}: intent write/sync: {e}"))?;
        sync_root(root, label)?;
        Ok(required_file_content_snapshot_v2(&file, label)?)
    }
    fn read_intent(
        root: &File,
        name: &ChildNameV2,
        label: &str,
    ) -> Result<(CustodyIntentV2, FileContentSnapshotV2, Option<CommitmentV2>), Failure> {
        let mut file = open_regular_child(root, name.as_os_str(), label)?;
        if file.metadata().map_err(|e| e.to_string())?.len() > INTENT_CAP {
            return Err("intent exceeds cap".to_owned().into());
        }
        let snap = required_file_content_snapshot_v2(&file, label)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let (intent, commitment) = decode(&bytes)?;
        Ok((intent, snap, commitment))
    }
    fn clear_intent<F>(
        op: &JournalRootOperationV2<'_>,
        intent: &CustodyIntentV2,
        snap: FileContentSnapshotV2,
        hook: &mut F,
        label: &str,
    ) -> Result<File, Failure>
    where
        F: FnMut(TransitionV2, &ChildNameV2) -> bool,
    {
        let name = intent.reserved_name(ReservedNameNamespaceV2::TransactionIntent);
        let held = remove(op, name, snap, None, hook, label)?;
        if hook(TransitionV2::IntentRemoved, name) {
            return Err(format!("{label}: interrupted after intent removal").into());
        }
        sync_root(op.root_file(), label)?;
        if hook(TransitionV2::FinalSync, name) {
            return Err(format!("{label}: interrupted after final sync").into());
        }
        op.prove_route_v2(label)?;
        Ok(held)
    }
    fn sha256(mut file: &File, label: &str) -> Result<CommitmentV2, Failure> {
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = [0; 8192];
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|error| FsCustodyError::Io(label.into(), error))?;
            if count == 0 {
                return Ok(context.finish().as_ref().try_into().expect("SHA-256"));
            }
            context.update(&buffer[..count]);
        }
    }
    fn verify_target(
        root: &File,
        name: &ChildNameV2,
        expected: FileContentSnapshotV2,
        commitment: CommitmentV2,
        label: &str,
    ) -> Result<File, Failure> {
        let (file, before) =
            snapshot(root, name, label)?.ok_or_else(|| format!("{label}: target absent"))?;
        if before != expected {
            return Err(format!("{label}: target changed").into());
        }
        file.sync_all()
            .map_err(|e| format!("{label}: target sync: {e}"))?;
        if required_file_content_snapshot_v2(&file, label)? != expected {
            return Err(format!("{label}: target changed during sync").into());
        }
        if sha256(&file, label)? != commitment {
            return Err(format!("{label}: target commitment changed").into());
        }
        Ok(file)
    }
    fn rollback_unsupported(
        op: &JournalRootOperationV2<'_>,
        label: &str,
        reason: String,
    ) -> NamespaceTransactionOutcomeV2 {
        match NamespaceTransactionV2::recover(op, label) {
            NamespaceTransactionOutcomeV2::Ready
            | NamespaceTransactionOutcomeV2::NoEffect(_, _) => {
                NamespaceTransactionOutcomeV2::Unsupported(reason)
            }
            _ => NamespaceTransactionOutcomeV2::ProtectiveDebt(format!(
                "{label}: unsupported capture rollback failed"
            )),
        }
    }
    impl NamespaceTransactionV2 {
        pub fn replace(
            op: &JournalRootOperationV2<'_>,
            target: ChildNameV2,
            expected: RequiredObjectIdentityV2,
            successor: &[u8],
            label: &str,
        ) -> NamespaceTransactionOutcomeV2 {
            Self::replace_with(op, target, expected, successor, label, |_, _| false)
        }
        pub fn retire(
            op: &JournalRootOperationV2<'_>,
            target: ChildNameV2,
            expected: RequiredObjectIdentityV2,
            label: &str,
        ) -> NamespaceTransactionOutcomeV2 {
            Self::retire_with(op, target, expected, label, |_, _| false)
        }
        pub fn recover(
            op: &JournalRootOperationV2<'_>,
            label: &str,
        ) -> NamespaceTransactionOutcomeV2 {
            Self::recover_with(op, label, |_, _| false)
        }
        fn ready(
            op: &JournalRootOperationV2<'_>,
            footprint: usize,
            label: &str,
        ) -> Option<NamespaceTransactionOutcomeV2> {
            match op.guard(None, footprint, label) {
                Ok(()) => None,
                Err(JournalMutationOutcomeV2::Unsupported(reason)) => {
                    Some(NamespaceTransactionOutcomeV2::Unsupported(reason))
                }
                Err(reason) => Some(NamespaceTransactionOutcomeV2::ProtectiveDebt(format!(
                    "{label}: {reason:?}"
                ))),
            }
        }
        fn replace_with<F>(
            op: &JournalRootOperationV2<'_>,
            target: ChildNameV2,
            expected: RequiredObjectIdentityV2,
            successor: &[u8],
            label: &str,
            hook: F,
        ) -> NamespaceTransactionOutcomeV2
        where
            F: FnMut(TransitionV2, &ChildNameV2) -> bool,
        {
            record(
                op,
                Self::replace_run(op, target, expected, successor, label, hook),
            )
        }
        fn replace_run<F>(
            op: &JournalRootOperationV2<'_>,
            target: ChildNameV2,
            expected: RequiredObjectIdentityV2,
            successor: &[u8],
            label: &str,
            mut hook: F,
        ) -> NamespaceTransactionOutcomeV2
        where
            F: FnMut(TransitionV2, &ChildNameV2) -> bool,
        {
            let recovery = ticket(CustodyOperationKindV2::Replace, &target);
            if let Some(value) = Self::ready(op, 2, label) {
                return value;
            }
            if target.is_reserved_target() {
                return NamespaceTransactionOutcomeV2::NoEffect(
                    recovery,
                    format!("{label}: reserved target"),
                );
            }
            let root = op.root_file();
            match snapshot(root, &target, label) {
                Ok(Some((_, value))) if value.object == expected => {}
                Ok(_) => {
                    return NamespaceTransactionOutcomeV2::NoEffect(
                        recovery,
                        format!("{label}: predecessor precondition refused"),
                    )
                }
                Err(reason) => return debt(recovery, reason),
            }
            let stage_name = match ChildNameV2::reserved(ReservedNameNamespaceV2::Staging, &target)
            {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            if let Err(error) = op.prove_route_v2(label) {
                return debt(recovery, error);
            }
            let mut stage = match create_new_regular_child_at(root, stage_name.as_os_str(), label) {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            if let Err(e) = stage.write_all(successor).and_then(|_| stage.sync_all()) {
                return debt(recovery, format!("{label}: stage write/sync: {e}"));
            }
            let staged = match required_file_content_snapshot_v2(&stage, label) {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            let commitment: CommitmentV2 = ring::digest::digest(&ring::digest::SHA256, successor)
                .as_ref()
                .try_into()
                .expect("SHA-256");
            if let Err(e) = sync_root(root, label) {
                return debt(recovery, e);
            }
            if hook(TransitionV2::StageCreated, &stage_name) {
                return debt(
                    recovery,
                    format!("{label}: interrupted after stage creation"),
                );
            }
            let intent = match CustodyIntentV2::new(
                CustodyOperationKindV2::Replace,
                target,
                expected,
                staged,
            ) {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            let intent_snap = match create_intent(op, &intent, Some(commitment), label) {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            let intent_name = intent.reserved_name(ReservedNameNamespaceV2::TransactionIntent);
            if hook(TransitionV2::IntentSynced, intent_name) {
                return debt(recovery, format!("{label}: interrupted after intent sync"));
            }
            hook(TransitionV2::BeforeCapture, intent.parts().1);
            if let Err(error) = op.prove_route_v2(label) {
                return debt(recovery, error);
            }
            match capture(root, &intent, label) {
                CustodyCaptureOutcomeV2::ExpectedCaptured(_) => {}
                CustodyCaptureOutcomeV2::CompileUnsupported => {
                    return rollback_unsupported(op, label, label.into())
                }
                CustodyCaptureOutcomeV2::RuntimeUnsupported(reason) => {
                    return rollback_unsupported(op, label, reason)
                }
                _ => {
                    let _ = sync_root(root, label);
                    return Self::recover_with(op, label, hook);
                }
            }
            if let Err(e) = sync_root(root, label) {
                return debt(recovery, e);
            }
            if hook(TransitionV2::Captured, intent.capture_name()) {
                return debt(recovery, format!("{label}: interrupted after capture"));
            }
            if !matches!(snapshot(root, intent.reserved_name(ReservedNameNamespaceV2::Staging), label), Ok(Some((_, value))) if value == staged)
            {
                return debt(
                    recovery,
                    format!("{label}: staged identity/content changed"),
                );
            }
            if let Err(e) = move_new(
                op,
                intent.reserved_name(ReservedNameNamespaceV2::Staging),
                intent.parts().1,
                label,
            ) {
                return debt(recovery, e);
            }
            if hook(TransitionV2::Published, intent.parts().1) {
                return debt(recovery, format!("{label}: committed target retains debt"));
            }
            if let Err(e) = verify_target(root, intent.parts().1, staged, commitment, label) {
                return debt(recovery, e);
            }
            if hook(TransitionV2::TargetSynced, intent.parts().1) {
                return debt(recovery, format!("{label}: committed target retains debt"));
            }
            Self::finish(
                op,
                &intent,
                intent_snap,
                Some((staged, commitment)),
                &mut hook,
                label,
            )
        }
        fn retire_with<F>(
            op: &JournalRootOperationV2<'_>,
            target: ChildNameV2,
            expected: RequiredObjectIdentityV2,
            label: &str,
            hook: F,
        ) -> NamespaceTransactionOutcomeV2
        where
            F: FnMut(TransitionV2, &ChildNameV2) -> bool,
        {
            record(op, Self::retire_run(op, target, expected, label, hook))
        }
        fn retire_run<F>(
            op: &JournalRootOperationV2<'_>,
            target: ChildNameV2,
            expected: RequiredObjectIdentityV2,
            label: &str,
            mut hook: F,
        ) -> NamespaceTransactionOutcomeV2
        where
            F: FnMut(TransitionV2, &ChildNameV2) -> bool,
        {
            let recovery = ticket(CustodyOperationKindV2::Retire, &target);
            if let Some(value) = Self::ready(op, 1, label) {
                return value;
            }
            if target.is_reserved_target() {
                return NamespaceTransactionOutcomeV2::NoEffect(
                    recovery,
                    format!("{label}: reserved target"),
                );
            }
            let root = op.root_file();
            let observed = match snapshot(root, &target, label) {
                Ok(Some((_, v))) if v.object == expected => v,
                Ok(_) => {
                    return NamespaceTransactionOutcomeV2::NoEffect(
                        recovery,
                        format!("{label}: predecessor precondition refused"),
                    )
                }
                Err(e) => return debt(recovery, e),
            };
            let intent = match CustodyIntentV2::new(
                CustodyOperationKindV2::Retire,
                target,
                expected,
                observed,
            ) {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            let intent_snap = match create_intent(op, &intent, None, label) {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            if hook(
                TransitionV2::IntentSynced,
                intent.reserved_name(ReservedNameNamespaceV2::TransactionIntent),
            ) {
                return debt(recovery, format!("{label}: interrupted after intent sync"));
            }
            hook(TransitionV2::BeforeCapture, intent.parts().1);
            if let Err(error) = op.prove_route_v2(label) {
                return debt(recovery, error);
            }
            match capture(root, &intent, label) {
                CustodyCaptureOutcomeV2::ExpectedCaptured(_) => {}
                CustodyCaptureOutcomeV2::CompileUnsupported => {
                    return rollback_unsupported(op, label, label.into())
                }
                CustodyCaptureOutcomeV2::RuntimeUnsupported(reason) => {
                    return rollback_unsupported(op, label, reason)
                }
                _ => {
                    let _ = sync_root(root, label);
                    return Self::recover_with(op, label, hook);
                }
            }
            if let Err(e) = sync_root(root, label) {
                return debt(recovery, e);
            }
            if hook(TransitionV2::Captured, intent.capture_name()) {
                return debt(recovery, format!("{label}: interrupted after capture"));
            }
            Self::finish(op, &intent, intent_snap, None, &mut hook, label)
        }
        fn finish<F>(
            op: &JournalRootOperationV2<'_>,
            intent: &CustodyIntentV2,
            intent_snap: FileContentSnapshotV2,
            target_expectation: Option<(FileContentSnapshotV2, CommitmentV2)>,
            hook: &mut F,
            label: &str,
        ) -> NamespaceTransactionOutcomeV2
        where
            F: FnMut(TransitionV2, &ChildNameV2) -> bool,
        {
            let recovery = ticket(intent.parts().0, intent.parts().1);
            let capture = intent.capture_name();
            let expected = match snapshot(op.root_file(), capture, label) {
                Ok(Some((_, v))) if v.object == *intent.parts().2 => v,
                Ok(_) => {
                    return debt(
                        recovery,
                        format!("{label}: predecessor capture is not exact"),
                    )
                }
                Err(e) => return debt(recovery, e),
            };
            if let Some((staged, commitment)) = target_expectation {
                if let Err(e) =
                    verify_target(op.root_file(), intent.parts().1, staged, commitment, label)
                {
                    return debt(recovery, e);
                }
            }
            if let Err(e) = remove(
                op,
                capture,
                expected,
                Some((TransitionV2::Retired, TransitionV2::ZeroLinkProved)),
                hook,
                label,
            ) {
                return debt(recovery, e);
            }
            if let Err(e) = clear_intent(op, intent, intent_snap, hook, label) {
                return debt(recovery, e);
            }
            NamespaceTransactionOutcomeV2::Complete(recovery)
        }
        fn recover_with<F>(
            op: &JournalRootOperationV2<'_>,
            label: &str,
            hook: F,
        ) -> NamespaceTransactionOutcomeV2
        where
            F: FnMut(TransitionV2, &ChildNameV2) -> bool,
        {
            record(op, Self::recover_run(op, label, hook))
        }
        fn recover_run<F>(
            op: &JournalRootOperationV2<'_>,
            label: &str,
            mut hook: F,
        ) -> NamespaceTransactionOutcomeV2
        where
            F: FnMut(TransitionV2, &ChildNameV2) -> bool,
        {
            if !cfg!(any(target_os = "linux", target_os = "macos")) {
                return NamespaceTransactionOutcomeV2::Unsupported(label.into());
            }
            if let Err(e) = op.prove_route_v2(label) {
                return protect(e);
            }
            let root = op.root_file();
            let names = match enumerate_directory_names(root, CHILD_CAP, label) {
                Ok(v) => v,
                Err(e) => return protect(e),
            };
            let mut reserved = Vec::new();
            let mut intents = Vec::new();
            for name in names {
                if !name.as_bytes().starts_with(b".a2a-v2-") {
                    continue;
                }
                let name = match ChildNameV2::from_bytes(name.as_bytes()) {
                    Ok(v) => v,
                    Err(_) => {
                        return NamespaceTransactionOutcomeV2::ProtectiveDebt(format!(
                            "{label}: malformed residue"
                        ))
                    }
                };
                if ChildNameV2::parse_reserved(ReservedNameNamespaceV2::TransactionIntent, &name)
                    .is_ok()
                {
                    intents.push(name.clone());
                }
                reserved.push(name);
            }
            if reserved.is_empty() {
                if let Err(error) = sync_root(root, label) {
                    return protect(error);
                }
                if let Err(error) = op.prove_route_v2(label) {
                    return protect(error);
                }
                op.clear_protective_debt();
                return NamespaceTransactionOutcomeV2::Ready;
            }
            if intents.len() != 1 {
                return NamespaceTransactionOutcomeV2::ProtectiveDebt(format!(
                    "{label}: missing or duplicate intent"
                ));
            }
            let (intent, intent_snap, commitment) = match read_intent(root, &intents[0], label) {
                Ok(v) => v,
                Err(e) => return protect(e),
            };
            let expected_names: Vec<_> = ReservedNameNamespaceV2::ALL
                .map(|n| intent.reserved_name(n))
                .into_iter()
                .collect();
            if intents[0] != *intent.reserved_name(ReservedNameNamespaceV2::TransactionIntent)
                || reserved.iter().any(|n| !expected_names.contains(&n))
            {
                return NamespaceTransactionOutcomeV2::ProtectiveDebt(format!(
                    "{label}: foreign or ambiguous residue"
                ));
            }
            let recovery = ticket(intent.parts().0, intent.parts().1);
            let state = |name| snapshot(root, name, label).map(|v| v.map(|(_, s)| s));
            let target = match state(intent.parts().1) {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            let stage = match state(intent.reserved_name(ReservedNameNamespaceV2::Staging)) {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            let swap =
                match state(intent.reserved_name(ReservedNameNamespaceV2::ReplacementCapture)) {
                    Ok(v) => v,
                    Err(e) => return debt(recovery, e),
                };
            let del = match state(intent.reserved_name(ReservedNameNamespaceV2::RetirementCapture))
            {
                Ok(v) => v,
                Err(e) => return debt(recovery, e),
            };
            match intent.parts().0 {
                CustodyOperationKindV2::Replace => {
                    let commitment = commitment.expect("validated replacement commitment");
                    if del.is_some() {
                        return debt(recovery, format!("{label}: foreign retirement residue"));
                    }
                    if target.map(|v| v.object) == Some(*intent.parts().2) && swap.is_none() {
                        if let Some(v) = stage {
                            if v != *intent.parts().3 {
                                return debt(recovery, format!("{label}: stage changed"));
                            }
                            if let Err(e) = remove(
                                op,
                                intent.reserved_name(ReservedNameNamespaceV2::Staging),
                                v,
                                None,
                                &mut hook,
                                label,
                            ) {
                                return debt(recovery, e);
                            }
                        }
                        if let Err(e) = clear_intent(op, &intent, intent_snap, &mut hook, label) {
                            return debt(recovery, e);
                        }
                        return NamespaceTransactionOutcomeV2::NoEffect(
                            recovery,
                            format!("{label}: predecessor restored"),
                        );
                    }
                    if target.is_none()
                        && swap.map(|v| v.object) == Some(*intent.parts().2)
                        && stage == Some(*intent.parts().3)
                    {
                        if let Err(e) = move_new(
                            op,
                            intent.reserved_name(ReservedNameNamespaceV2::ReplacementCapture),
                            intent.parts().1,
                            label,
                        ) {
                            return debt(recovery, e);
                        }
                        return Self::recover_with(op, label, hook);
                    }
                    if target == Some(*intent.parts().3)
                        && swap.map(|v| v.object) == Some(*intent.parts().2)
                        && stage.is_none()
                    {
                        if let Err(e) = verify_target(
                            root,
                            intent.parts().1,
                            *intent.parts().3,
                            commitment,
                            label,
                        ) {
                            return debt(recovery, e);
                        }
                        if hook(TransitionV2::TargetSynced, intent.parts().1) {
                            return debt(recovery, format!("{label}: recovery interrupted"));
                        }
                        return Self::finish(
                            op,
                            &intent,
                            intent_snap,
                            Some((*intent.parts().3, commitment)),
                            &mut hook,
                            label,
                        );
                    }
                    if target.is_none()
                        && swap.is_some()
                        && swap.map(|v| v.object) != Some(*intent.parts().2)
                    {
                        if let Err(e) = move_new(
                            op,
                            intent.reserved_name(ReservedNameNamespaceV2::ReplacementCapture),
                            intent.parts().1,
                            label,
                        ) {
                            return debt(recovery, e);
                        }
                    }
                    debt(recovery, format!("{label}: replacement residue retained"))
                }
                CustodyOperationKindV2::Retire => {
                    if stage.is_some() || swap.is_some() {
                        return debt(recovery, format!("{label}: foreign replacement residue"));
                    }
                    if target.map(|v| v.object) == Some(*intent.parts().2) && del.is_none() {
                        if let Err(e) = clear_intent(op, &intent, intent_snap, &mut hook, label) {
                            return debt(recovery, e);
                        }
                        return NamespaceTransactionOutcomeV2::NoEffect(
                            recovery,
                            format!("{label}: retirement had no effect"),
                        );
                    }
                    if target.is_none() && del.map(|v| v.object) == Some(*intent.parts().2) {
                        return Self::finish(op, &intent, intent_snap, None, &mut hook, label);
                    }
                    debt(recovery, format!("{label}: retirement residue retained"))
                }
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::fs_custody::{
            required_object_identity_v2, BirthTimeV1, JournalMutationOutcomeV2,
            JournalRootBindingV2, JournalRootCustodyV2, ObjectIdentityV2,
        };
        use std::{
            fs,
            path::{Path, PathBuf},
        };
        use NamespaceTransactionOutcomeV2 as O;

        struct Case {
            _dir: tempfile::TempDir,
            anchor: PathBuf,
            root: PathBuf,
            binding: JournalRootBindingV2,
        }
        fn object(path: &Path) -> ObjectIdentityV2 {
            let metadata = fs::metadata(path).unwrap();
            required_object_identity_v2(
                metadata.dev(),
                metadata.ino(),
                BirthTimeV1::from_metadata(&metadata),
                "fixture",
            )
            .unwrap()
        }
        fn case() -> Case {
            let dir = tempfile::tempdir().unwrap();
            let anchor = dir.path().join("anchor");
            let parent = anchor.join("parent");
            let root = parent.join("journal");
            let lock = parent.join("operation.lock");
            fs::create_dir_all(&root).unwrap();
            fs::write(&lock, b"").unwrap();
            fs::write(root.join("target"), b"A").unwrap();
            let binding = JournalRootBindingV2::new(
                object(&anchor),
                ChildNameV2::from_bytes(b"parent").unwrap(),
                object(&parent),
                ChildNameV2::from_bytes(b"journal").unwrap(),
                object(&root),
                ChildNameV2::from_bytes(b"operation.lock").unwrap(),
                object(&lock),
            )
            .unwrap();
            Case {
                _dir: dir,
                anchor,
                root,
                binding,
            }
        }
        impl Case {
            fn operate<T>(&self, run: impl FnOnce(&JournalRootOperationV2<'_>) -> T) -> T {
                let custody =
                    JournalRootCustodyV2::open(&self.anchor, &self.binding, "fixture").unwrap();
                let operation = custody.begin_operation("fixture").unwrap();
                run(&operation)
            }
            fn expected(&self) -> RequiredObjectIdentityV2 {
                required_file_content_snapshot_v2(
                    &File::open(self.root.join("target")).unwrap(),
                    "A",
                )
                .unwrap()
                .object
            }
        }
        fn target() -> ChildNameV2 {
            ChildNameV2::from_bytes(b"target").unwrap()
        }
        fn reserved(namespace: ReservedNameNamespaceV2) -> ChildNameV2 {
            ChildNameV2::reserved(namespace, &target()).unwrap()
        }
        fn residue(case: &Case) -> Vec<String> {
            let mut names: Vec<_> = fs::read_dir(&case.root)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .filter(|name| name.starts_with(".a2a-v2-"))
                .collect();
            names.sort();
            names
        }

        fn fill_root_to(case: &Case, count: usize) {
            let source = case.root.join("ordinary-1");
            fs::write(&source, b"x").unwrap();
            for index in 2..count {
                fs::hard_link(&source, case.root.join(format!("ordinary-{index}"))).unwrap();
            }
        }

        #[test]
        fn namespace_transaction_happy_replace_and_retire_complete_only_after_cleanup() {
            let case = case();
            let outcome = case.operate(|op| {
                NamespaceTransactionV2::replace(op, target(), case.expected(), b"B", "replace")
            });
            assert!(matches!(
                outcome,
                NamespaceTransactionOutcomeV2::Complete(_)
            ));
            assert_eq!(fs::read(case.root.join("target")).unwrap(), b"B");
            assert!(residue(&case).is_empty());
            let expected = case.expected();
            let outcome =
                case.operate(|op| NamespaceTransactionV2::retire(op, target(), expected, "retire"));
            assert!(matches!(
                outcome,
                NamespaceTransactionOutcomeV2::Complete(_)
            ));
            assert!(!case.root.join("target").exists());
            assert!(residue(&case).is_empty());
        }

        #[test]
        fn namespace_transaction_recovery_debt_blocks_every_mutator_until_clean() {
            let case = case();
            let expected = case.expected();
            assert!(matches!(
                case.operate(|op| NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"B",
                    "plant debt",
                    |transition, _| transition == TransitionV2::IntentSynced,
                )),
                O::Retained(_, _)
            ));
            let target_snapshot = required_file_content_snapshot_v2(
                &File::open(case.root.join("target")).unwrap(),
                "target",
            )
            .unwrap();
            let staged = required_file_content_snapshot_v2(
                &File::open(
                    case.root
                        .join(reserved(ReservedNameNamespaceV2::Staging).as_os_str()),
                )
                .unwrap(),
                "stage",
            )
            .unwrap();
            let recovered = case.operate(|op| {
                let other = ChildNameV2::from_bytes(b"other").unwrap();
                for outcome in [
                    op.stage(&other, b"x", "blocked"),
                    op.publish(&target(), staged, "blocked"),
                    op.append(&target(), target_snapshot, 1, b"x", "blocked"),
                ] {
                    assert!(matches!(
                        outcome,
                        Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
                    ));
                }
                assert!(matches!(
                    op.sync("blocked"),
                    JournalMutationOutcomeV2::ProtectiveDebt(_)
                ));
                for outcome in [
                    NamespaceTransactionV2::replace(op, target(), expected, b"x", "blocked"),
                    NamespaceTransactionV2::retire(op, target(), expected, "blocked"),
                ] {
                    assert!(matches!(outcome, O::ProtectiveDebt(_)));
                }
                NamespaceTransactionV2::recover(op, "recover debt")
            });
            assert!(matches!(recovered, O::NoEffect(_, _)));
            assert_eq!(
                case.operate(|op| NamespaceTransactionV2::recover(op, "clean")),
                O::Ready
            );
        }

        #[test]
        fn namespace_transaction_residue_free_retained_blocks_same_handle_until_clean_recovery() {
            let case = case();
            let custody = JournalRootCustodyV2::open(&case.anchor, &case.binding, "debt").unwrap();
            let op = custody.begin_operation("debt").unwrap();
            let retained = NamespaceTransactionV2::replace_with(
                &op,
                target(),
                case.expected(),
                b"B",
                "debt",
                |transition, _| transition == TransitionV2::FinalSync,
            );
            assert!(matches!(record(&op, retained), O::Retained(_, _)));
            assert!(residue(&case).is_empty());
            let next = ChildNameV2::from_bytes(b"next").unwrap();
            assert!(matches!(
                op.stage(&next, b"x", "blocked"),
                Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
            ));
            assert_eq!(NamespaceTransactionV2::recover(&op, "clean"), O::Ready);
            assert!(op.stage(&next, b"x", "after recovery").is_ok());
        }

        #[test]
        fn namespace_transaction_reserves_replace_and_retire_capacity_before_effect() {
            for count in [4095, 4096] {
                let case = case();
                fill_root_to(&case, count);
                let replaced = case.operate(|op| {
                    NamespaceTransactionV2::replace(
                        op,
                        target(),
                        case.expected(),
                        b"B",
                        "replace capacity",
                    )
                });
                assert!(matches!(replaced, O::ProtectiveDebt(_)));
                assert!(residue(&case).is_empty());
                assert_eq!(fs::read_dir(&case.root).unwrap().count(), count);
                let retired = case.operate(|op| {
                    NamespaceTransactionV2::retire(op, target(), case.expected(), "retire capacity")
                });
                let admitted = count == 4095;
                assert_eq!(matches!(retired, O::Complete(_)), admitted);
                if !admitted {
                    assert!(matches!(retired, O::ProtectiveDebt(_)));
                    assert!(residue(&case).is_empty());
                }
                assert_eq!(
                    fs::read_dir(&case.root).unwrap().count(),
                    count - usize::from(admitted)
                );
            }
        }

        #[test]
        fn namespace_transaction_reserved_targets_refuse_before_effect() {
            for target in [
                ".a2a-v2-int-target",
                ".a2a-v2-stg-target",
                ".a2a-v2-rpc-target",
                ".a2a-v2-rtc-target",
                ".a2a-v2-x",
            ] {
                let make_case = case;
                let case = case();
                fs::rename(case.root.join("target"), case.root.join(target)).unwrap();
                let name = ChildNameV2::from_bytes(target.as_bytes()).unwrap();
                let expected = required_file_content_snapshot_v2(
                    &File::open(case.root.join(target)).unwrap(),
                    "reserved target",
                )
                .unwrap()
                .object;
                let outcomes = case.operate(|op| {
                    [
                        NamespaceTransactionV2::replace(
                            op,
                            name.clone(),
                            expected,
                            b"B",
                            "reserved target",
                        ),
                        NamespaceTransactionV2::retire(op, name, expected, "reserved target"),
                    ]
                });
                for outcome in outcomes {
                    // A reserved-named object present in the root is residue by
                    // definition, so admission refuses protectively before the
                    // name-level refusal can apply; the root is untouched.
                    assert!(matches!(outcome, O::ProtectiveDebt(_)), "{outcome:?}");
                    assert_eq!(fs::read_dir(&case.root).unwrap().count(), 1);
                    assert_eq!(fs::read(case.root.join(target)).unwrap(), b"A");
                }

                // With no such object present, the pure name refusal applies
                // without any filesystem effect.
                let clean = make_case();
                let expected = clean.expected();
                let name = ChildNameV2::from_bytes(target.as_bytes()).unwrap();
                let outcomes = clean.operate(|op| {
                    [
                        NamespaceTransactionV2::replace(
                            op,
                            name.clone(),
                            expected,
                            b"B",
                            "reserved name",
                        ),
                        NamespaceTransactionV2::retire(op, name, expected, "reserved name"),
                    ]
                });
                for outcome in outcomes {
                    assert!(matches!(
                        outcome,
                        O::NoEffect(_, ref reason) if reason.contains("reserved target")
                    ));
                    assert_eq!(fs::read_dir(&clean.root).unwrap().count(), 1);
                    assert_eq!(fs::read(clean.root.join("target")).unwrap(), b"A");
                }
            }
        }

        #[test]
        fn namespace_transaction_unsupported_is_typed_and_clean_when_knowable() {
            for retire in [false, true] {
                let case = case();
                let expected = case.expected();
                SNAPSHOT_UNSUPPORTED.with(|fault| fault.set(true));
                let outcome = case.operate(|op| {
                    if retire {
                        NamespaceTransactionV2::retire(op, target(), expected, "u")
                    } else {
                        NamespaceTransactionV2::replace(op, target(), expected, b"B", "u")
                    }
                });
                assert!(matches!(outcome, O::Unsupported(_)));
                assert_eq!(fs::read(case.root.join("target")).unwrap(), b"A");
                assert!(residue(&case).is_empty());
            }
            let case = case();
            CAPTURE_UNSUPPORTED.with(|fault| fault.set(true));
            let outcome = case.operate(|op| {
                NamespaceTransactionV2::replace(op, target(), case.expected(), b"B", "u")
            });
            assert!(matches!(outcome, O::Unsupported(_)));
            assert_eq!(fs::read(case.root.join("target")).unwrap(), b"A");
            assert!(residue(&case).is_empty());
        }
        #[test]
        fn namespace_transaction_capture_substitution_is_restored_and_never_published() {
            let case = case();
            let expected = case.expected();
            let root = case.root.clone();
            let outcome = case.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"successor",
                    "capture substitution",
                    |transition, _| {
                        if transition == TransitionV2::BeforeCapture {
                            fs::rename(root.join("target"), root.join("A")).unwrap();
                            fs::write(root.join("target"), b"B").unwrap();
                        }
                        false
                    },
                )
            });
            assert!(
                matches!(outcome, NamespaceTransactionOutcomeV2::Retained(_, _)),
                "{outcome:?}"
            );
            assert_eq!(fs::read(case.root.join("target")).unwrap(), b"B");
            assert!(matches!(
                case.operate(|op| NamespaceTransactionV2::recover(op, "again")),
                NamespaceTransactionOutcomeV2::Retained(_, _)
            ));
        }

        #[test]
        fn namespace_transaction_takeover_and_stage_substitution_refuse_publication() {
            let takeover = case();
            let expected = takeover.expected();
            let root = takeover.root.clone();
            let outcome = takeover.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"successor",
                    "takeover",
                    |transition, _| {
                        if transition == TransitionV2::Captured {
                            fs::write(root.join("target"), b"takeover").unwrap();
                        }
                        false
                    },
                )
            });
            assert!(matches!(
                outcome,
                NamespaceTransactionOutcomeV2::Retained(_, _)
            ));
            assert_eq!(fs::read(takeover.root.join("target")).unwrap(), b"takeover");
            assert_eq!(
                fs::read(
                    takeover
                        .root
                        .join(reserved(ReservedNameNamespaceV2::ReplacementCapture).as_os_str())
                )
                .unwrap(),
                b"A"
            );

            let stage = case();
            let expected = stage.expected();
            let root = stage.root.clone();
            let outcome = stage.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"successor",
                    "stage substitution",
                    |transition, _| {
                        if transition == TransitionV2::Captured {
                            let name = reserved(ReservedNameNamespaceV2::Staging);
                            fs::rename(root.join(name.as_os_str()), root.join("old-stage"))
                                .unwrap();
                            fs::write(root.join(name.as_os_str()), b"foreign").unwrap();
                        }
                        false
                    },
                )
            });
            assert!(matches!(
                outcome,
                NamespaceTransactionOutcomeV2::Retained(_, _)
            ));
            assert!(!stage.root.join("target").exists());

            let live = case();
            let root = live.root.clone();
            let outcome = live.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    live.expected(),
                    b"B",
                    "c",
                    |transition, _| {
                        if transition == TransitionV2::Published {
                            fs::write(root.join("target"), b"C").unwrap();
                        }
                        false
                    },
                )
            });
            assert!(matches!(outcome, O::Retained(_, _)));
            assert_eq!(fs::read(live.root.join("target")).unwrap(), b"C");
            let swap = reserved(ReservedNameNamespaceV2::ReplacementCapture);
            assert_eq!(fs::read(live.root.join(swap.as_os_str())).unwrap(), b"A");
        }

        #[test]
        fn namespace_transaction_reserved_substitution_blocks_exact_cleanup() {
            let case = case();
            let expected = case.expected();
            let root = case.root.clone();
            let outcome = case.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"B",
                    "cleanup substitution",
                    |transition, name| {
                        if transition == TransitionV2::BeforeCleanup
                            && ChildNameV2::parse_reserved(
                                ReservedNameNamespaceV2::ReplacementCapture,
                                name,
                            )
                            .is_ok()
                        {
                            fs::rename(root.join(name.as_os_str()), root.join("saved-A")).unwrap();
                            fs::write(root.join(name.as_os_str()), b"foreign").unwrap();
                        }
                        false
                    },
                )
            });
            assert!(matches!(
                outcome,
                NamespaceTransactionOutcomeV2::Retained(_, _)
            ));
            assert_eq!(fs::read(case.root.join("target")).unwrap(), b"B");
            assert_eq!(fs::read(case.root.join("saved-A")).unwrap(), b"A");
            assert_eq!(
                fs::read(
                    case.root
                        .join(reserved(ReservedNameNamespaceV2::ReplacementCapture).as_os_str())
                )
                .unwrap(),
                b"foreign"
            );
        }

        #[test]
        fn namespace_transaction_every_replace_crash_cut_has_a_stable_recovery_outcome() {
            let cuts = [
                TransitionV2::StageCreated,
                TransitionV2::IntentSynced,
                TransitionV2::Captured,
                TransitionV2::Published,
                TransitionV2::TargetSynced,
                TransitionV2::Retired,
                TransitionV2::ZeroLinkProved,
                TransitionV2::IntentRemoved,
                TransitionV2::FinalSync,
            ];
            for cut in cuts {
                let case = case();
                let expected = case.expected();
                let interrupted = case.operate(|op| {
                    NamespaceTransactionV2::replace_with(
                        op,
                        target(),
                        expected,
                        b"B",
                        "crash cut",
                        |transition, _| transition == cut,
                    )
                });
                assert!(matches!(
                    interrupted,
                    NamespaceTransactionOutcomeV2::Retained(_, _)
                ));
                if cut == TransitionV2::Published {
                    fs::write(case.root.join("target"), b"C").unwrap();
                }
                let recovered =
                    case.operate(|op| NamespaceTransactionV2::recover(op, "recover cut"));
                match cut {
                    TransitionV2::StageCreated => assert!(matches!(
                        recovered,
                        NamespaceTransactionOutcomeV2::ProtectiveDebt(_)
                    )),
                    TransitionV2::IntentSynced | TransitionV2::Captured => assert!(
                        matches!(recovered, NamespaceTransactionOutcomeV2::NoEffect(_, _)),
                        "{cut:?}: {recovered:?}"
                    ),
                    TransitionV2::Published => {
                        assert!(matches!(recovered, O::Retained(_, _)));
                        let swap = reserved(ReservedNameNamespaceV2::ReplacementCapture);
                        assert_eq!(fs::read(case.root.join(swap.as_os_str())).unwrap(), b"A");
                    }
                    TransitionV2::TargetSynced => assert!(matches!(
                        recovered,
                        NamespaceTransactionOutcomeV2::Complete(_)
                    )),
                    TransitionV2::Retired | TransitionV2::ZeroLinkProved => assert!(matches!(
                        recovered,
                        NamespaceTransactionOutcomeV2::Retained(_, _)
                    )),
                    TransitionV2::IntentRemoved | TransitionV2::FinalSync => {
                        assert_eq!(recovered, NamespaceTransactionOutcomeV2::Ready)
                    }
                    _ => unreachable!(),
                }
                let stable = case.operate(|op| NamespaceTransactionV2::recover(op, "repeat"));
                match cut {
                    TransitionV2::StageCreated => assert!(matches!(
                        stable,
                        NamespaceTransactionOutcomeV2::ProtectiveDebt(_)
                    )),
                    TransitionV2::Published
                    | TransitionV2::Retired
                    | TransitionV2::ZeroLinkProved => assert!(matches!(stable, O::Retained(_, _))),
                    _ => assert_eq!(stable, NamespaceTransactionOutcomeV2::Ready),
                }
            }
        }

        #[test]
        fn namespace_transaction_replace_rollback_and_every_retire_crash_cut_are_pinned() {
            let replace = case();
            let expected = replace.expected();
            let _ = replace.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"B",
                    "replace crash",
                    |transition, _| transition == TransitionV2::Captured,
                )
            });
            let recovered =
                replace.operate(|op| NamespaceTransactionV2::recover(op, "replace recovery"));
            assert!(
                matches!(recovered, NamespaceTransactionOutcomeV2::NoEffect(_, _)),
                "{recovered:?}"
            );
            assert_eq!(fs::read(replace.root.join("target")).unwrap(), b"A");

            for cut in [
                TransitionV2::IntentSynced,
                TransitionV2::Captured,
                TransitionV2::Retired,
                TransitionV2::ZeroLinkProved,
                TransitionV2::IntentRemoved,
                TransitionV2::FinalSync,
            ] {
                let retire = case();
                let expected = retire.expected();
                let interrupted = retire.operate(|op| {
                    NamespaceTransactionV2::retire_with(
                        op,
                        target(),
                        expected,
                        "r",
                        |transition, _| transition == cut,
                    )
                });
                assert!(matches!(interrupted, O::Retained(_, _)));
                let recovered = retire.operate(|op| NamespaceTransactionV2::recover(op, "r"));
                assert!(
                    match cut {
                        TransitionV2::IntentSynced => matches!(recovered, O::NoEffect(_, _)),
                        TransitionV2::Captured => matches!(recovered, O::Complete(_)),
                        TransitionV2::Retired | TransitionV2::ZeroLinkProved => {
                            matches!(recovered, O::Retained(_, _))
                        }
                        TransitionV2::IntentRemoved | TransitionV2::FinalSync =>
                            recovered == O::Ready,
                        _ => unreachable!(),
                    },
                    "{cut:?}: {recovered:?}"
                );
                if matches!(cut, TransitionV2::Retired | TransitionV2::ZeroLinkProved) {
                    let stable = retire.operate(|op| NamespaceTransactionV2::recover(op, "r"));
                    assert!(matches!(stable, O::Retained(_, _)), "{stable:?}");
                }
            }
        }

        #[test]
        fn namespace_transaction_malformed_duplicate_foreign_and_over_cap_residue_is_preserved() {
            let malformed = case();
            let path = malformed.root.join(".a2a-v2-int-bad");
            fs::write(&path, b"{").unwrap();
            assert!(matches!(
                malformed.operate(|op| NamespaceTransactionV2::recover(op, "malformed")),
                NamespaceTransactionOutcomeV2::ProtectiveDebt(_)
            ));
            assert!(path.exists());

            let duplicate = case();
            let expected = duplicate.expected();
            let _ = duplicate.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"B",
                    "duplicate setup",
                    |transition, _| transition == TransitionV2::IntentSynced,
                )
            });
            let first = duplicate
                .root
                .join(reserved(ReservedNameNamespaceV2::TransactionIntent).as_os_str());
            let second = duplicate.root.join(".a2a-v2-int-other");
            fs::write(&second, fs::read(&first).unwrap()).unwrap();
            assert!(matches!(
                duplicate.operate(|op| NamespaceTransactionV2::recover(op, "duplicate")),
                NamespaceTransactionOutcomeV2::ProtectiveDebt(_)
            ));
            assert!(first.exists() && second.exists());

            let foreign = case();
            let path = foreign.root.join(".a2a-v2-stg-foreign");
            fs::write(&path, b"foreign").unwrap();
            assert!(matches!(
                foreign.operate(|op| NamespaceTransactionV2::recover(op, "foreign")),
                NamespaceTransactionOutcomeV2::ProtectiveDebt(_)
            ));
            assert!(path.exists());

            let over = case();
            for index in 0..CHILD_CAP {
                fs::write(over.root.join(format!("child-{index}")), b"x").unwrap();
            }
            assert!(matches!(
                over.operate(|op| NamespaceTransactionV2::recover(op, "over cap")),
                NamespaceTransactionOutcomeV2::ProtectiveDebt(_)
            ));
            assert!(over.root.join("child-0").exists());
        }

        #[test]
        fn namespace_transaction_post_digest_target_mutation_refuses_completion() {
            let case = case();
            let expected = case.expected();
            let root = case.root.clone();
            let outcome = case.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"B",
                    "post-digest mutation",
                    |transition, _| {
                        if transition == TransitionV2::TargetSynced {
                            fs::write(root.join("target"), b"C").unwrap();
                        }
                        false
                    },
                )
            });
            assert!(
                matches!(outcome, NamespaceTransactionOutcomeV2::Retained(_, _)),
                "{outcome:?}"
            );
            assert_eq!(fs::read(case.root.join("target")).unwrap(), b"C");
            assert_eq!(
                fs::read(
                    case.root
                        .join(reserved(ReservedNameNamespaceV2::ReplacementCapture).as_os_str())
                )
                .unwrap(),
                b"A"
            );
        }

        #[test]
        fn namespace_transaction_recovery_rechecks_target_before_finish() {
            let case = case();
            let expected = case.expected();
            let _ = case.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"B",
                    "recovery setup",
                    |transition, _| transition == TransitionV2::Published,
                )
            });
            let root = case.root.clone();
            let outcome = case.operate(|op| {
                NamespaceTransactionV2::recover_with(op, "recovery recheck", |transition, _| {
                    if transition == TransitionV2::TargetSynced {
                        fs::write(root.join("target"), b"C").unwrap();
                    }
                    false
                })
            });
            assert!(matches!(outcome, O::Retained(_, _)), "{outcome:?}");
            assert_eq!(fs::read(case.root.join("target")).unwrap(), b"C");
            let capture = reserved(ReservedNameNamespaceV2::ReplacementCapture);
            assert_eq!(fs::read(case.root.join(capture.as_os_str())).unwrap(), b"A");
        }

        #[test]
        fn namespace_transaction_recovery_unsupported_identity_is_typed() {
            let case = case();
            let expected = case.expected();
            let _ = case.operate(|op| {
                NamespaceTransactionV2::replace_with(
                    op,
                    target(),
                    expected,
                    b"B",
                    "unsupported setup",
                    |transition, _| transition == TransitionV2::IntentSynced,
                )
            });
            assert!(!residue(&case).is_empty());
            SNAPSHOT_UNSUPPORTED.with(|fault| fault.set(true));
            let outcome =
                case.operate(|op| NamespaceTransactionV2::recover(op, "unsupported recovery"));
            assert!(
                matches!(outcome, NamespaceTransactionOutcomeV2::Unsupported(_)),
                "{outcome:?}"
            );
            assert!(!residue(&case).is_empty());
            assert!(matches!(
                case.operate(|op| NamespaceTransactionV2::recover(op, "after")),
                NamespaceTransactionOutcomeV2::NoEffect(_, _)
            ));
        }

        #[test]
        fn namespace_transaction_missing_birthtime_identity_classifies_unsupported() {
            let error = required_object_identity_v2(1, 2, None, "missing birthtime").unwrap_err();
            let outcome = debt(ticket(CustodyOperationKindV2::Replace, &target()), error);
            assert!(
                matches!(outcome, NamespaceTransactionOutcomeV2::Unsupported(_)),
                "{outcome:?}"
            );
            let error = required_object_identity_v2(1, 2, None, "missing birthtime").unwrap_err();
            assert!(matches!(
                protect(error),
                NamespaceTransactionOutcomeV2::Unsupported(_)
            ));
        }

        #[test]
        fn namespace_transaction_wire_rejects_mismatched_commitment_presence() {
            for (operation, commitment) in [
                (CustodyOperationKindV2::Replace, None),
                (CustodyOperationKindV2::Retire, Some([0u8; 32])),
            ] {
                let case = case();
                let staged = required_file_content_snapshot_v2(
                    &File::open(case.root.join("target")).unwrap(),
                    "wire fixture",
                )
                .unwrap();
                let intent =
                    CustodyIntentV2::new(operation, target(), staged.object, staged).unwrap();
                let bytes = serde_json::to_vec(&to_wire(&intent, commitment)).unwrap();
                fs::write(
                    case.root.join(
                        intent
                            .reserved_name(ReservedNameNamespaceV2::TransactionIntent)
                            .as_os_str(),
                    ),
                    bytes,
                )
                .unwrap();
                let outcome =
                    case.operate(|op| NamespaceTransactionV2::recover(op, "wire negative"));
                assert!(
                    matches!(
                        outcome,
                        NamespaceTransactionOutcomeV2::ProtectiveDebt(ref reason)
                            if reason.contains("invalid staged commitment")
                    ),
                    "{outcome:?}"
                );
                assert!(!residue(&case).is_empty());
            }
        }

        #[test]
        fn namespace_reserved_target_on_debted_handle_stays_protective_until_recovery() {
            let case = case();
            let expected = case.expected();
            for index in 0..4095 {
                fs::write(case.root.join(format!("ordinary-{index}")), b"x").unwrap();
            }
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "debted").unwrap();
            let operation = custody.begin_operation("debted").unwrap();
            assert!(matches!(
                operation.stage(&ChildNameV2::from_bytes(b"extra").unwrap(), b"x", "cap"),
                Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
            ));
            assert!(operation.debt());
            let reserved = ChildNameV2::from_bytes(b".a2a-v2-x").unwrap();
            let outcome = NamespaceTransactionV2::replace(
                &operation,
                reserved.clone(),
                expected,
                b"B",
                "reserved on debt",
            );
            assert!(matches!(outcome, O::ProtectiveDebt(_)), "{outcome:?}");
            assert!(operation.debt());
            assert!(matches!(
                NamespaceTransactionV2::recover(&operation, "clean pass"),
                O::Ready
            ));
            assert!(!operation.debt());
            for index in 0..8 {
                fs::remove_file(case.root.join(format!("ordinary-{index}"))).unwrap();
            }
            let outcome = NamespaceTransactionV2::replace(
                &operation,
                reserved,
                expected,
                b"B",
                "reserved after recovery",
            );
            assert!(matches!(outcome, O::NoEffect(_, _)), "{outcome:?}");
            assert!(!operation.debt());
        }

        #[test]
        fn namespace_retained_outcome_records_write_blocking_debt() {
            let case = case();
            let expected = case.expected();
            let root = case.root.clone();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "retained debt").unwrap();
            let operation = custody.begin_operation("retained debt").unwrap();
            let outcome = NamespaceTransactionV2::replace_with(
                &operation,
                target(),
                expected,
                b"successor",
                "retained debt",
                |transition, _| {
                    if transition == TransitionV2::BeforeCapture {
                        fs::rename(root.join("target"), root.join("A")).unwrap();
                        fs::write(root.join("target"), b"B").unwrap();
                    }
                    false
                },
            );
            assert!(matches!(outcome, O::Retained(_, _)), "{outcome:?}");
            assert!(
                operation.debt(),
                "a transaction Retained outcome must record write-blocking debt"
            );
            assert!(matches!(
                operation.stage(
                    &ChildNameV2::from_bytes(b"blocked").unwrap(),
                    b"x",
                    "blocked"
                ),
                Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
            ));
        }

        #[test]
        fn namespace_transaction_a3_api_is_present_without_success_flattening() {
            let _: Option<NamespaceTransactionV2> = None;
            let _: Option<NamespaceRecoveryTicketV2> = None;
            let _: Option<NamespaceTransactionOutcomeV2> = None;
        }
    }
}
pub use mechanism::{
    NamespaceRecoveryTicketV2, NamespaceTransactionOutcomeV2, NamespaceTransactionV2,
};
