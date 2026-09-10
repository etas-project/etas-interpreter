use crate::host::{HostFuture, HostServiceAvailability, HostServices};
use etas_effects::HostRequirementKind;
use etas_host::console::{ConsoleOperation, ConsoleRequest, ConsoleResponse, ConsoleResult};
use etas_host::{
    ApprovalDecision, ApprovalRequest, ApprovalResponse, BrowserProtocolRequest,
    BrowserProtocolResponse, ByteStreamOrigin, CommandOutput, CommandRequest, CommandResponse,
    FilesystemRequest, FilesystemResponse, HostError, HostErrorCode, HostValue, MemoryConflict,
    MemoryOperation, MemoryRequest, MemoryResponse, MemoryResult, MemoryVersion, ModelContent,
    ModelMessage, ModelRequest, ModelResponse, ModelRole, ModelToolCall, PolicyDecision,
    PolicyEvaluationRequest, PolicyResponse, SecretRequest, SecretResponse, SessionRequest,
    SessionResponse, StreamFailure, StreamRequest, StreamResponse, TcpConnectRequest,
    TcpConnectResponse, TcpStreamRef, TlsConnectRequest, TlsConnectResponse, ToolRequest,
    ToolResponse, WriteCondition,
};
use etas_host::{StreamPayload, StreamRead};
use std::collections::{HashMap, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Clone, Debug)]
struct FakeMemoryEntry {
    key: HostValue,
    value: HostValue,
}

pub(super) enum TestMemoryBackend {
    Volatile(etas_host::InMemoryMemoryClient),
    Sqlite(etas_host::SqliteMemoryClient),
    ReplaceBeforeDelete(etas_host::InMemoryMemoryClient),
    LostWriteResponse {
        client: etas_host::InMemoryMemoryClient,
        inner_error: bool,
    },
    WrongWriteTarget {
        client: etas_host::InMemoryMemoryClient,
        other_store: bool,
    },
}

#[derive(Clone, Copy)]
pub(super) enum SessionContextFault {
    LostResponse,
    ForeignReceipt,
}

pub(super) struct FakeHost {
    pub(super) session_context_fault: Option<SessionContextFault>,
    pub(super) storage: Option<TestMemoryBackend>,
    console_gate: Option<Arc<ConsoleGate>>,
    availability: HostServiceAvailability,
    approval_calls: Arc<AtomicUsize>,
    approval_decisions: Arc<Mutex<VecDeque<ApprovalDecision>>>,
    approval_responses: Arc<Mutex<VecDeque<ApprovalResponse>>>,
    command_calls: Arc<AtomicUsize>,
    console_calls: Arc<AtomicUsize>,
    memory_calls: Arc<AtomicUsize>,
    session_calls: Arc<AtomicUsize>,
    model_calls: Arc<AtomicUsize>,
    policy_calls: Arc<AtomicUsize>,
    command_requests: Arc<Mutex<Vec<CommandRequest>>>,
    command_response: Arc<Mutex<Option<CommandOutput>>>,
    model_requests: Arc<Mutex<Vec<ModelRequest>>>,
    model_errors: Arc<Mutex<VecDeque<HostError>>>,
    model_responses: Arc<Mutex<VecDeque<ModelResponse>>>,
    preserve_model_response_id: Arc<AtomicBool>,
    policy_requests: Arc<Mutex<Vec<PolicyEvaluationRequest>>>,
    policy_decisions: Arc<Mutex<VecDeque<PolicyDecision>>>,
    policy_decision: Arc<Mutex<PolicyDecision>>,
    tool_requests: Arc<Mutex<Vec<ToolRequest>>>,
    tool_responses: Arc<Mutex<VecDeque<Result<HostValue, HostError>>>>,
    tool_response: Arc<Mutex<Option<HostValue>>>,
    tcp_requests: Arc<Mutex<Vec<TcpConnectRequest>>>,
    tcp_responses: Arc<Mutex<VecDeque<Result<TcpStreamRef, HostError>>>>,
    stream_requests: Arc<Mutex<Vec<StreamRequest>>>,
    stream_responses: Arc<Mutex<VecDeque<Result<StreamPayload, StreamFailure>>>>,
    console_error: Arc<Mutex<Option<HostError>>>,
    stdin: Arc<Mutex<String>>,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
    memory: Arc<Mutex<HashMap<String, FakeMemoryEntry>>>,
    memory_conflicts: Arc<Mutex<VecDeque<MemoryConflict>>>,
    pub(super) session: etas_host::InMemorySessionClient,
    pub(super) persistent_session: Option<etas_host::SqliteSessionClient>,
    pub(super) lose_session_append_response: bool,
    pub(super) lose_session_resolve_response: bool,
}

impl FakeHost {
    pub(super) fn new(availability: HostServiceAvailability) -> Self {
        Self {
            storage: None,
            console_gate: None,
            availability,
            approval_calls: Arc::new(AtomicUsize::new(0)),
            approval_decisions: Arc::new(Mutex::new(VecDeque::new())),
            approval_responses: Arc::new(Mutex::new(VecDeque::new())),
            command_calls: Arc::new(AtomicUsize::new(0)),
            console_calls: Arc::new(AtomicUsize::new(0)),
            memory_calls: Arc::new(AtomicUsize::new(0)),
            session_calls: Arc::new(AtomicUsize::new(0)),
            session_context_fault: None,
            model_calls: Arc::new(AtomicUsize::new(0)),
            policy_calls: Arc::new(AtomicUsize::new(0)),
            command_requests: Arc::new(Mutex::new(Vec::new())),
            command_response: Arc::new(Mutex::new(None)),
            model_requests: Arc::new(Mutex::new(Vec::new())),
            model_errors: Arc::new(Mutex::new(VecDeque::new())),
            model_responses: Arc::new(Mutex::new(VecDeque::new())),
            preserve_model_response_id: Arc::new(AtomicBool::new(false)),
            policy_requests: Arc::new(Mutex::new(Vec::new())),
            policy_decisions: Arc::new(Mutex::new(VecDeque::new())),
            policy_decision: Arc::new(Mutex::new(PolicyDecision::Allow)),
            tool_requests: Arc::new(Mutex::new(Vec::new())),
            tool_responses: Arc::new(Mutex::new(VecDeque::new())),
            tool_response: Arc::new(Mutex::new(None)),
            tcp_requests: Arc::new(Mutex::new(Vec::new())),
            tcp_responses: Arc::new(Mutex::new(VecDeque::new())),
            stream_requests: Arc::new(Mutex::new(Vec::new())),
            stream_responses: Arc::new(Mutex::new(VecDeque::new())),
            console_error: Arc::new(Mutex::new(None)),
            stdin: Arc::new(Mutex::new(String::new())),
            stdout: Arc::new(Mutex::new(String::new())),
            stderr: Arc::new(Mutex::new(String::new())),
            memory: Arc::new(Mutex::new(HashMap::new())),
            memory_conflicts: Arc::new(Mutex::new(VecDeque::new())),
            session: etas_host::InMemorySessionClient::new(),
            persistent_session: None,
            lose_session_append_response: false,
            lose_session_resolve_response: false,
        }
    }

    pub(super) fn approval_call_count(&self) -> usize {
        self.approval_calls.load(Ordering::SeqCst)
    }

    pub(super) fn pause_console_completion(&mut self) -> Arc<ConsoleGate> {
        let gate = Arc::new(ConsoleGate {
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        self.console_gate = Some(gate.clone());
        gate
    }

    pub(super) fn seed_approval_decision(&self, decision: ApprovalDecision) {
        self.approval_decisions
            .lock()
            .expect("approval decisions lock")
            .push_back(decision);
    }

    pub(super) fn seed_approval_response(&self, response: ApprovalResponse) {
        self.approval_responses
            .lock()
            .expect("approval responses lock")
            .push_back(response);
    }

    pub(super) fn memory_call_count(&self) -> usize {
        self.memory_calls.load(Ordering::SeqCst)
    }

    pub(super) fn session_call_count(&self) -> usize {
        self.session_calls.load(Ordering::SeqCst)
    }

    pub(super) fn console_call_count(&self) -> usize {
        self.console_calls.load(Ordering::SeqCst)
    }

    pub(super) fn command_call_count(&self) -> usize {
        self.command_calls.load(Ordering::SeqCst)
    }

    pub(super) fn command_requests(&self) -> Vec<CommandRequest> {
        self.command_requests
            .lock()
            .expect("command requests lock")
            .clone()
    }

    pub(super) fn model_call_count(&self) -> usize {
        self.model_calls.load(Ordering::SeqCst)
    }

    pub(super) fn policy_call_count(&self) -> usize {
        self.policy_calls.load(Ordering::SeqCst)
    }

    pub(super) fn model_requests(&self) -> Vec<ModelRequest> {
        self.model_requests
            .lock()
            .expect("model requests lock")
            .clone()
    }

    pub(super) fn policy_requests(&self) -> Vec<PolicyEvaluationRequest> {
        self.policy_requests
            .lock()
            .expect("policy requests lock")
            .clone()
    }

    pub(super) fn deny_policy(&self, reason: &str) {
        *self.policy_decision.lock().expect("policy decision lock") = PolicyDecision::Deny {
            reason: reason.to_owned(),
        };
    }

    pub(super) fn seed_policy_decision(&self, decision: PolicyDecision) {
        self.policy_decisions
            .lock()
            .expect("policy decisions lock")
            .push_back(decision);
    }

    pub(super) fn tool_requests(&self) -> Vec<ToolRequest> {
        self.tool_requests
            .lock()
            .expect("tool requests lock")
            .clone()
    }

    pub(super) fn seed_model_response_text(&self, text: &str) {
        self.seed_model_response_text_with_usage(
            text,
            Some(etas_host::ModelUsage {
                input_tokens: 1,
                output_tokens: 1,
                cost: None,
            }),
        );
    }

    pub(super) fn seed_model_response_text_with_usage(
        &self,
        text: &str,
        usage: Option<etas_host::ModelUsage>,
    ) {
        self.model_responses
            .lock()
            .expect("model responses lock")
            .push_back(ModelResponse {
                id: etas_host::HostRequestId(0),
                message: ModelMessage {
                    role: ModelRole::Assistant,
                    content: vec![ModelContent::Text(text.to_owned())],
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                },
                tool_calls: Vec::new(),
                usage,
            });
    }

    pub(super) fn seed_model_error(&self, code: HostErrorCode, message: &str) {
        self.model_errors
            .lock()
            .expect("model errors lock")
            .push_back(HostError::new(code, message));
    }

    pub(super) fn force_next_model_response_id(&self, id: etas_host::HostRequestId) {
        self.model_responses
            .lock()
            .expect("model responses lock")
            .front_mut()
            .expect("next model response")
            .id = id;
        self.preserve_model_response_id
            .store(true, Ordering::SeqCst);
    }

    pub(super) fn seed_model_response_tool_call(&self, tool: &str) {
        self.seed_model_response_tool_call_with_args(tool, HostValue::Record(Vec::new()));
    }

    pub(super) fn seed_model_response_tool_call_with_args(&self, tool: &str, args: HostValue) {
        self.model_responses
            .lock()
            .expect("model responses lock")
            .push_back(ModelResponse {
                id: etas_host::HostRequestId(0),
                message: ModelMessage {
                    role: ModelRole::Assistant,
                    content: Vec::new(),
                    tool_call_id: None,
                    tool_calls: vec![ModelToolCall {
                        id: "call-1".to_owned(),
                        tool: tool.to_owned(),
                        args: args.clone(),
                    }],
                },
                tool_calls: vec![ModelToolCall {
                    id: "call-1".to_owned(),
                    tool: tool.to_owned(),
                    args,
                }],
                usage: Some(etas_host::ModelUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cost: None,
                }),
            });
    }

    #[allow(dead_code)]
    pub(super) fn seed_tool_response_value(&self, value: HostValue) {
        *self.tool_response.lock().expect("tool response lock") = Some(value);
    }

    #[allow(dead_code)]
    pub(super) fn seed_tool_response_once(&self, value: HostValue) {
        self.tool_responses
            .lock()
            .expect("tool responses lock")
            .push_back(Ok(value));
    }

    #[allow(dead_code)]
    pub(super) fn seed_tool_error(&self, code: HostErrorCode, message: &str) {
        self.tool_responses
            .lock()
            .expect("tool responses lock")
            .push_back(Err(HostError::new(code, message)));
    }

    pub(super) fn stream_requests(&self) -> Vec<StreamRequest> {
        self.stream_requests
            .lock()
            .expect("stream requests lock")
            .clone()
    }

    pub(super) fn tcp_requests(&self) -> Vec<TcpConnectRequest> {
        self.tcp_requests.lock().expect("tcp requests lock").clone()
    }

    pub(super) fn seed_tcp_connect_stream(&self, id: &str, host: &str, port: u16) {
        self.tcp_responses
            .lock()
            .expect("tcp responses lock")
            .push_back(Ok(TcpStreamRef::issued(
                etas_host::StreamHandleRef::issued(id, 0),
                ByteStreamOrigin::Tcp {
                    host: host.to_owned(),
                    port,
                },
            )));
    }

    pub(super) fn seed_tcp_connect_error(&self, code: HostErrorCode, message: &str) {
        self.tcp_responses
            .lock()
            .expect("tcp responses lock")
            .push_back(Err(HostError::new(code, message)));
    }

    pub(super) fn seed_stream_read_until_limit_failure(&self, failure: StreamFailure) {
        self.stream_responses
            .lock()
            .expect("stream responses lock")
            .push_back(Err(failure));
    }

    #[allow(dead_code)]
    pub(super) fn seed_stream_read_data(&self, bytes: &[u8]) {
        self.stream_responses
            .lock()
            .expect("stream responses lock")
            .push_back(Ok(StreamPayload::Read(StreamRead::Data(bytes.to_vec()))));
    }

    pub(super) fn seed_command_output(&self, exit_code: i32, stdout: &[u8], stderr: &[u8]) {
        *self.command_response.lock().expect("command response lock") = Some(CommandOutput {
            isolation: etas_host::CommandIsolationReport::trusted_unconfined(),
            exit_code,
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        });
    }

    pub(super) fn seed_stdin(&self, input: &str) {
        *self.stdin.lock().expect("stdin lock") = input.to_owned();
    }

    pub(super) fn fail_console(&self, code: HostErrorCode, message: &str) {
        *self.console_error.lock().expect("console error lock") =
            Some(HostError::new(code, message));
    }

    pub(super) fn stdout_text(&self) -> String {
        self.stdout.lock().expect("stdout lock").clone()
    }

    pub(super) fn stderr_text(&self) -> String {
        self.stderr.lock().expect("stderr lock").clone()
    }

    pub(super) fn seed_memory(
        &self,
        region: &str,
        path: &[&str],
        key: HostValue,
        value: HostValue,
    ) {
        self.memory.lock().expect("memory lock").insert(
            memory_key(region, path, &key),
            FakeMemoryEntry { key, value },
        );
    }

    pub(super) fn seed_memory_conflict(&self, conflict: MemoryConflict) {
        self.memory_conflicts
            .lock()
            .expect("memory conflicts lock")
            .push_back(conflict);
    }

    pub(super) fn memory_value(
        &self,
        region: &str,
        path: &[&str],
        key: &HostValue,
    ) -> Option<HostValue> {
        self.memory
            .lock()
            .expect("memory lock")
            .get(&memory_key(region, path, key))
            .map(|entry| entry.value.clone())
    }
}

impl HostServices for FakeHost {
    fn availability(&self) -> HostServiceAvailability {
        self.availability
    }

    fn model<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: ModelRequest,
    ) -> HostFuture<'a, Result<ModelResponse, HostError>> {
        self.model_calls.fetch_add(1, Ordering::SeqCst);
        self.model_requests
            .lock()
            .expect("model requests lock")
            .push(request.clone());
        let error = self
            .model_errors
            .lock()
            .expect("model errors lock")
            .pop_front();
        if let Some(error) = error {
            return Box::pin(async move { Err(error) });
        }
        let response = self
            .model_responses
            .lock()
            .expect("model responses lock")
            .pop_front();
        let preserve_response_id = self.preserve_model_response_id.load(Ordering::SeqCst);
        Box::pin(async move {
            let Some(mut response) = response else {
                return Err(HostError::new(
                    etas_host::HostErrorCode::ProviderUnavailable,
                    "test model response is not configured",
                ));
            };
            if !preserve_response_id {
                response.id = request.id;
            }
            Ok(response)
        })
    }

    fn tool<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: ToolRequest,
    ) -> HostFuture<'a, Result<ToolResponse, HostError>> {
        self.tool_requests
            .lock()
            .expect("tool requests lock")
            .push(request.clone());
        let queued = self
            .tool_responses
            .lock()
            .expect("tool responses lock")
            .pop_front();
        let value = self
            .tool_response
            .lock()
            .expect("tool response lock")
            .clone();
        Box::pin(async move {
            if let Some(result) = queued {
                return result.map(|value| ToolResponse {
                    id: request.id,
                    result: Ok(value),
                });
            }
            let Some(value) = value else {
                return Err(HostError::new(
                    HostErrorCode::ProviderUnavailable,
                    "test tool response is not configured",
                ));
            };
            Ok(ToolResponse {
                id: request.id,
                result: Ok(value),
            })
        })
    }

    fn memory<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: MemoryRequest,
    ) -> HostFuture<'a, Result<MemoryResponse, HostError>> {
        self.memory_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(storage) = &self.storage {
            return Box::pin(async move {
                match storage {
                    TestMemoryBackend::Volatile(client)
                    | TestMemoryBackend::WrongWriteTarget { client, .. } => {
                        client.execute_scoped(request, &_operation).await
                    }
                    TestMemoryBackend::Sqlite(client) => {
                        client.execute_scoped(request, &_operation).await
                    }
                    TestMemoryBackend::ReplaceBeforeDelete(client) => {
                        if let MemoryOperation::Delete { key, .. } = &request.operation {
                            let mut replacement = request.clone();
                            replacement.operation = MemoryOperation::Put {
                                key: key.clone(),
                                value: HostValue::String("concurrent replacement".to_owned()),
                                condition: WriteCondition::Any,
                            };
                            client
                                .execute_scoped(replacement, &_operation)
                                .await?
                                .result?;
                        }
                        client.execute_scoped(request, &_operation).await
                    }
                    TestMemoryBackend::LostWriteResponse {
                        client,
                        inner_error,
                    } => {
                        let write = matches!(
                            request.operation,
                            MemoryOperation::Put { .. } | MemoryOperation::Delete { .. }
                        );
                        let response = client.execute_scoped(request, &_operation).await?;
                        if write && response.result.is_ok() {
                            let error = HostError::new(
                                HostErrorCode::ProviderUnavailable,
                                "lost write acknowledgement",
                            );
                            if *inner_error {
                                Ok(MemoryResponse {
                                    id: response.id,
                                    result: Err(error),
                                })
                            } else {
                                Err(error)
                            }
                        } else {
                            Ok(response)
                        }
                    }
                }
            });
        }
        let memory = Arc::clone(&self.memory);
        let conflict = self
            .memory_conflicts
            .lock()
            .expect("memory conflicts lock")
            .pop_front();
        Box::pin(async move {
            if let Some(conflict) = conflict {
                return Ok(MemoryResponse {
                    id: request.id,
                    result: Ok(MemoryResult::Conflict(conflict)),
                });
            }
            let result = match request.operation.clone() {
                MemoryOperation::Get { key } => memory
                    .lock()
                    .expect("memory lock")
                    .get(&memory_key(
                        &request.store.region.stable_id,
                        &request
                            .store
                            .path
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                        &key,
                    ))
                    .map(|entry| entry.value.clone())
                    .map(|value| MemoryResult::Value {
                        value,
                        version: fake_memory_version("v1"),
                    })
                    .unwrap_or(MemoryResult::None),
                MemoryOperation::Put {
                    key,
                    value,
                    condition,
                } => {
                    let key_string = memory_key(
                        &request.store.region.stable_id,
                        &request
                            .store
                            .path
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                        &key,
                    );
                    let mut memory = memory.lock().expect("memory lock");
                    if let Some(conflict) =
                        fake_memory_write_conflict(memory.get(&key_string), &condition)
                    {
                        return Ok(MemoryResponse {
                            id: request.id,
                            result: Ok(MemoryResult::Conflict(conflict)),
                        });
                    }
                    memory.insert(key_string, FakeMemoryEntry { key, value });
                    MemoryResult::Written {
                        version: fake_memory_version("v1"),
                    }
                }
                MemoryOperation::Delete { key, condition } => {
                    let key_string = memory_key(
                        &request.store.region.stable_id,
                        &request
                            .store
                            .path
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                        &key,
                    );
                    let mut memory = memory.lock().expect("memory lock");
                    if let Some(conflict) =
                        fake_memory_write_conflict(memory.get(&key_string), &condition)
                    {
                        return Ok(MemoryResponse {
                            id: request.id,
                            result: Ok(MemoryResult::Conflict(conflict)),
                        });
                    }
                    memory.remove(&key_string);
                    MemoryResult::Deleted {
                        version: fake_memory_version("v1"),
                    }
                }
                MemoryOperation::Scan { limit, .. } => {
                    let prefix = memory_prefix(
                        &request.store.region.stable_id,
                        &request
                            .store
                            .path
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                    );
                    let mut pairs = memory
                        .lock()
                        .expect("memory lock")
                        .iter()
                        .filter_map(|(encoded, entry)| {
                            encoded.strip_prefix(&prefix)?;
                            Some((entry.key.clone(), entry.value.clone()))
                        })
                        .collect::<Vec<_>>();
                    pairs.sort_by(|(left, _), (right, _)| {
                        format!("{left:?}").cmp(&format!("{right:?}"))
                    });
                    let entries = pairs
                        .into_iter()
                        .take(limit.unwrap_or(u32::MAX) as usize)
                        .map(|(key, value)| etas_host::MemoryEntry {
                            key,
                            value,
                            version: fake_memory_version("v1"),
                        })
                        .collect();
                    MemoryResult::Entries {
                        entries,
                        cursor: None,
                    }
                }
                MemoryOperation::Query { query, limit } => {
                    let prefix = memory_prefix(
                        &request.store.region.stable_id,
                        &request
                            .store
                            .path
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                    );
                    let predicate = query.predicate;
                    let mut entries = memory
                        .lock()
                        .expect("memory lock")
                        .iter()
                        .filter_map(|(encoded, entry)| {
                            encoded.strip_prefix(&prefix)?;
                            if !matches_predicate(&entry.key, &entry.value, predicate.as_ref()) {
                                return None;
                            }
                            Some(etas_host::MemoryEntry {
                                key: entry.key.clone(),
                                value: entry.value.clone(),
                                version: fake_memory_version("v1"),
                            })
                        })
                        .collect::<Vec<_>>();
                    entries.sort_by(|left, right| {
                        format!("{:?}", left.key).cmp(&format!("{:?}", right.key))
                    });
                    let entries = entries
                        .into_iter()
                        .take(limit.unwrap_or(u32::MAX) as usize)
                        .collect();
                    MemoryResult::Entries {
                        entries,
                        cursor: None,
                    }
                }
                MemoryOperation::VectorSearch {
                    embedding,
                    limit,
                    filter,
                } => {
                    let prefix = memory_prefix(
                        &request.store.region.stable_id,
                        &request
                            .store
                            .path
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                    );
                    let mut scored = memory
                        .lock()
                        .expect("memory lock")
                        .iter()
                        .filter_map(|(encoded, entry)| {
                            encoded.strip_prefix(&prefix)?;
                            if !matches_predicate(&entry.key, &entry.value, filter.as_ref()) {
                                return None;
                            }
                            let candidate = extract_embedding(&entry.value)?;
                            let score = cosine_similarity(&embedding, candidate)?;
                            Some((
                                score,
                                etas_host::MemoryEntry {
                                    key: entry.key.clone(),
                                    value: entry.value.clone(),
                                    version: fake_memory_version("v1"),
                                },
                            ))
                        })
                        .collect::<Vec<_>>();
                    scored.sort_by(|(left_score, left), (right_score, right)| {
                        right_score.total_cmp(left_score).then_with(|| {
                            format!("{:?}", left.key).cmp(&format!("{:?}", right.key))
                        })
                    });
                    let entries = scored
                        .into_iter()
                        .take(limit.max(1) as usize)
                        .map(|(_, entry)| entry)
                        .collect();
                    MemoryResult::Entries {
                        entries,
                        cursor: None,
                    }
                }
            };
            Ok(MemoryResponse {
                id: request.id,
                result: Ok(result),
            })
        })
    }

    fn memory_write<'a>(
        &'a self,
        operation: etas_host::execution::OperationContext,
        request: etas_host::memory::MemoryWriteRequest,
    ) -> HostFuture<'a, Result<etas_host::memory::MemoryWriteResponse, HostError>> {
        use etas_host::memory::*;
        use etas_host::{ReceiptLookup, StorageDurability, WriteOutcome};
        Box::pin(async move {
            if let Some(storage) = &self.storage {
                self.memory_calls.fetch_add(1, Ordering::SeqCst);
                return match storage {
                    TestMemoryBackend::Volatile(client) => {
                        client.write_scoped(request, &operation).await
                    }
                    TestMemoryBackend::Sqlite(client) => {
                        client.write_scoped(request, &operation).await
                    }
                    TestMemoryBackend::WrongWriteTarget {
                        client,
                        other_store,
                    } => {
                        let mut response = client.write_scoped(request, &operation).await?;
                        let Ok(MemoryWriteResult::Outcome(WriteOutcome::Committed(receipt))) =
                            &mut response.result
                        else {
                            panic!("test write must commit before acknowledgement is corrupted")
                        };
                        if *other_store {
                            receipt.target.store.path.push("other-store".into());
                        } else {
                            receipt.target.key = HostValue::String("other-key".into());
                        }
                        Ok(response)
                    }
                    TestMemoryBackend::ReplaceBeforeDelete(client) => {
                        if let MemoryWriteOperation::Mutate {
                            mutation: MemoryMutation::Delete { key, .. },
                            ..
                        } = &request.operation
                        {
                            client
                                .execute_scoped(
                                    MemoryRequest {
                                        id: request.id,
                                        store: request.store.clone(),
                                        operation: MemoryOperation::Put {
                                            key: key.clone(),
                                            value: HostValue::String(
                                                "concurrent replacement".into(),
                                            ),
                                            condition: WriteCondition::Any,
                                        },
                                        authority: request.authority.clone(),
                                        trace: request.trace.clone(),
                                        budget: request.budget.clone(),
                                    },
                                    &operation,
                                )
                                .await?
                                .result?;
                        }
                        client.write_scoped(request, &operation).await
                    }
                    TestMemoryBackend::LostWriteResponse {
                        client,
                        inner_error,
                    } => {
                        let write =
                            matches!(request.operation, MemoryWriteOperation::Mutate { .. });
                        let response = client.write_scoped(request, &operation).await?;
                        if !write {
                            return Ok(response);
                        }
                        let error = HostError::new(
                            HostErrorCode::ProviderUnavailable,
                            "lost write acknowledgement",
                        );
                        if *inner_error {
                            Ok(MemoryWriteResponse {
                                id: response.id,
                                result: Err(error),
                            })
                        } else {
                            Err(error)
                        }
                    }
                };
            }
            let MemoryWriteOperation::Mutate { key, mutation } = request.operation else {
                return Ok(MemoryWriteResponse {
                    id: request.id,
                    result: Ok(MemoryWriteResult::Receipt(ReceiptLookup::Unresolved)),
                });
            };
            let reference = mutation.operation_ref(
                &request.store,
                key,
                &etas_host::StorageLimits::default(),
            )?;
            let target = mutation.target(&request.store);
            let (kind, legacy) = match mutation {
                MemoryMutation::Put {
                    key,
                    value,
                    condition,
                } => (
                    MemoryMutationKind::Put,
                    MemoryOperation::Put {
                        key,
                        value,
                        condition,
                    },
                ),
                MemoryMutation::Delete { key, condition } => (
                    MemoryMutationKind::Delete,
                    MemoryOperation::Delete { key, condition },
                ),
            };
            let response = self
                .memory(
                    operation,
                    MemoryRequest {
                        id: request.id,
                        store: request.store,
                        operation: legacy,
                        authority: request.authority,
                        trace: request.trace,
                        budget: request.budget,
                    },
                )
                .await?;
            let result = response.result.map(|result| {
                MemoryWriteResult::Outcome(match result {
                    MemoryResult::Written { version } | MemoryResult::Deleted { version } => {
                        WriteOutcome::Committed(MemoryWriteReceipt {
                            operation: reference,
                            target,
                            change: match kind {
                                MemoryMutationKind::Put => {
                                    etas_host::memory::MemoryWriteChange::Written { version }
                                }
                                MemoryMutationKind::Delete => {
                                    etas_host::memory::MemoryWriteChange::Deleted {
                                        tombstone: version,
                                    }
                                }
                            },
                            durability: StorageDurability::Volatile,
                        })
                    }
                    MemoryResult::Unchanged => WriteOutcome::NotCommitted {
                        operation: reference,
                        reason: MemoryNotCommitted::Unchanged,
                    },
                    MemoryResult::Conflict(conflict) => WriteOutcome::NotCommitted {
                        operation: reference,
                        reason: MemoryNotCommitted::Conflict {
                            expected: conflict.expected,
                            actual: conflict.actual,
                            current_value: conflict.current_value,
                        },
                    },
                    _ => panic!("fake mutation returned read result"),
                })
            });
            Ok(MemoryWriteResponse {
                id: response.id,
                result,
            })
        })
    }

    fn session<'a>(
        &'a self,
        operation: etas_host::execution::OperationContext,
        request: SessionRequest,
    ) -> HostFuture<'a, Result<SessionResponse, HostError>> {
        self.session_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let response = match &self.persistent_session {
                Some(client) => client.execute_scoped(request, &operation).await,
                None => self.session.execute_scoped(request, &operation).await,
            }?;
            Ok(response)
        })
    }

    fn session_write<'a>(
        &'a self,
        operation: etas_host::execution::OperationContext,
        request: etas_host::session::SessionWriteRequest,
    ) -> HostFuture<'a, Result<etas_host::session::SessionWriteResponse, HostError>> {
        self.session_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let publication = matches!(
                request.operation,
                etas_host::session::SessionWriteOperation::PublishContext(_)
            );
            let lose_response = match &request.operation {
                etas_host::session::SessionWriteOperation::Append { .. } => {
                    self.lose_session_append_response
                }
                etas_host::session::SessionWriteOperation::Resolve { .. } => {
                    self.lose_session_resolve_response
                }
                _ => false,
            };
            let mut response = match &self.persistent_session {
                Some(client) => client.write_scoped(request, &operation).await,
                None => self.session.write_scoped(request, &operation).await,
            }?;
            if publication {
                match self.session_context_fault {
                    Some(SessionContextFault::LostResponse) => {
                        return Err(HostError::new(
                            HostErrorCode::ProviderUnavailable,
                            "injected lost publication response",
                        ));
                    }
                    Some(SessionContextFault::ForeignReceipt) => {
                        if let Ok(etas_host::session::SessionWriteResult::Context(
                            etas_host::WriteOutcome::Committed(receipt),
                        )) = &mut response.result
                        {
                            receipt.session.id = "foreign-session".into();
                        }
                    }
                    None => {}
                }
            }
            if lose_response {
                return Err(HostError::new(
                    etas_host::HostErrorCode::ProviderUnavailable,
                    "injected lost session mutation acknowledgement",
                ));
            }
            Ok(response)
        })
    }

    fn filesystem<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: FilesystemRequest,
    ) -> HostFuture<'a, Result<FilesystemResponse, HostError>> {
        Box::pin(async move {
            Err(HostError::new(
                HostErrorCode::ProviderUnavailable,
                "test filesystem response is not configured",
            )
            .with_detail("request_id", request.id.0.to_string()))
        })
    }

    fn command<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: CommandRequest,
    ) -> HostFuture<'a, Result<CommandResponse, HostError>> {
        self.command_calls.fetch_add(1, Ordering::SeqCst);
        self.command_requests
            .lock()
            .expect("command requests lock")
            .push(request.clone());
        let response = self
            .command_response
            .lock()
            .expect("command response lock")
            .clone();
        Box::pin(async move {
            let Some(output) = response else {
                return Err(HostError::new(
                    HostErrorCode::ProviderUnavailable,
                    "test command response is not configured",
                ));
            };
            Ok(CommandResponse {
                id: request.id,
                result: Ok(output),
            })
        })
    }

    fn tcp<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: TcpConnectRequest,
    ) -> HostFuture<'a, Result<TcpConnectResponse, HostError>> {
        self.tcp_requests
            .lock()
            .expect("tcp requests lock")
            .push(request.clone());
        let response = self
            .tcp_responses
            .lock()
            .expect("tcp responses lock")
            .pop_front();
        Box::pin(async move {
            let Some(result) = response else {
                return Err(HostError::new(
                    HostErrorCode::ProviderUnavailable,
                    "test TCP response is not configured",
                )
                .with_detail("request_id", request.id.0.to_string()));
            };
            Ok(TcpConnectResponse {
                id: request.id,
                result,
            })
        })
    }

    fn stream<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: StreamRequest,
    ) -> HostFuture<'a, Result<StreamResponse, HostError>> {
        self.stream_requests
            .lock()
            .expect("stream requests lock")
            .push(request.clone());
        let response = self
            .stream_responses
            .lock()
            .expect("stream responses lock")
            .pop_front();
        Box::pin(async move {
            let Some(result) = response else {
                return Err(HostError::new(
                    HostErrorCode::ProviderUnavailable,
                    "test stream response is not configured",
                )
                .with_detail("request_id", request.id.0.to_string()));
            };
            Ok(StreamResponse {
                id: request.id,
                result,
            })
        })
    }

    fn tls<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: TlsConnectRequest,
    ) -> HostFuture<'a, Result<TlsConnectResponse, HostError>> {
        Box::pin(async move {
            Err(HostError::new(
                HostErrorCode::ProviderUnavailable,
                "test TLS response is not configured",
            )
            .with_detail("request_id", request.id.0.to_string()))
        })
    }

    fn secret<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: SecretRequest,
    ) -> HostFuture<'a, Result<SecretResponse, HostError>> {
        Box::pin(async move {
            Err(HostError::new(
                HostErrorCode::ProviderUnavailable,
                "test secret response is not configured",
            )
            .with_detail("request_id", request.id.0.to_string()))
        })
    }

    fn browser<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: BrowserProtocolRequest,
    ) -> HostFuture<'a, Result<BrowserProtocolResponse, HostError>> {
        Box::pin(async move {
            Err(HostError::new(
                HostErrorCode::ProviderUnavailable,
                "test browser protocol response is not configured",
            )
            .with_detail("request_id", request.id.0.to_string()))
        })
    }

    fn console<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: ConsoleRequest,
    ) -> HostFuture<'a, Result<ConsoleResponse, HostError>> {
        self.console_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self
            .console_error
            .lock()
            .expect("console error lock")
            .clone()
        {
            return Box::pin(async move { Err(error) });
        }
        let stdin = Arc::clone(&self.stdin);
        let stdout = Arc::clone(&self.stdout);
        let stderr = Arc::clone(&self.stderr);
        let gate = self.console_gate.clone();
        Box::pin(async move {
            let result = match request.operation {
                ConsoleOperation::ReadAllStdin => {
                    ConsoleResult::Input(std::mem::take(&mut *stdin.lock().expect("stdin lock")))
                }
                ConsoleOperation::ReadLineStdin => {
                    let mut input = stdin.lock().expect("stdin lock");
                    let end = input.find('\n').map_or(input.len(), |index| index + 1);
                    let line = input.drain(..end).collect();
                    ConsoleResult::Input(line)
                }
                ConsoleOperation::WriteStdout { text, newline } => {
                    let mut buffer = stdout.lock().expect("stdout lock");
                    buffer.push_str(&text);
                    if newline {
                        buffer.push('\n');
                    }
                    ConsoleResult::Written
                }
                ConsoleOperation::WriteStderr { text, newline } => {
                    let mut buffer = stderr.lock().expect("stderr lock");
                    buffer.push_str(&text);
                    if newline {
                        buffer.push('\n');
                    }
                    ConsoleResult::Written
                }
            };
            if let Some(gate) = gate {
                gate.started.notify_one();
                gate.release
                    .acquire()
                    .await
                    .expect("test gate open")
                    .forget();
            }
            Ok(ConsoleResponse {
                id: request.id,
                result,
            })
        })
    }

    fn approval<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: ApprovalRequest,
    ) -> HostFuture<'a, Result<ApprovalResponse, HostError>> {
        self.approval_calls.fetch_add(1, Ordering::SeqCst);
        let response = self
            .approval_responses
            .lock()
            .expect("approval responses lock")
            .pop_front();
        let queued = self
            .approval_decisions
            .lock()
            .expect("approval decisions lock")
            .pop_front();
        Box::pin(async move {
            if let Some(response) = response {
                return Ok(response);
            }
            if let Some(decision) = queued {
                return Ok(ApprovalResponse {
                    id: request.id,
                    decision,
                });
            }
            Ok(ApprovalResponse {
                id: request.id,
                decision: ApprovalDecision::Approved {
                    grant: etas_host::ApprovalGrant {
                        id: request.id,
                        grants: request.requested_grants,
                    },
                },
            })
        })
    }

    fn policy<'a>(
        &'a self,
        _operation: etas_host::execution::OperationContext,
        request: PolicyEvaluationRequest,
    ) -> HostFuture<'a, Result<PolicyResponse, HostError>> {
        self.policy_calls.fetch_add(1, Ordering::SeqCst);
        self.policy_requests
            .lock()
            .expect("policy requests lock")
            .push(request.clone());
        let decision = self
            .policy_decisions
            .lock()
            .expect("policy decisions lock")
            .pop_front()
            .unwrap_or_else(|| {
                self.policy_decision
                    .lock()
                    .expect("policy decision lock")
                    .clone()
            });
        Box::pin(async move {
            Ok(PolicyResponse {
                id: request.id,
                decision,
            })
        })
    }
}

pub(super) struct ConsoleGate {
    pub started: tokio::sync::Notify,
    pub release: tokio::sync::Semaphore,
}

pub(super) fn availability(kinds: &[HostRequirementKind]) -> HostServiceAvailability {
    let mut availability = HostServiceAvailability::default();
    for kind in kinds {
        availability.enable(*kind);
    }
    availability
}

fn memory_key(region: &str, path: &[&str], key: &HostValue) -> String {
    format!("{region}:{}:{key:?}", path.join("."))
}

pub(super) fn fake_memory_version(label: &str) -> MemoryVersion {
    let digest = blake3::hash(label.as_bytes());
    let revision = match label {
        "v1" => 1,
        "v2" => 2,
        _ => 3,
    };
    MemoryVersion::parse(&format!(
        "mv1:{}:{}:{revision:016x}",
        digest.to_hex(),
        "0".repeat(32)
    ))
    .expect("valid test version")
}
fn fake_memory_write_conflict(
    actual: Option<&FakeMemoryEntry>,
    condition: &WriteCondition,
) -> Option<MemoryConflict> {
    let actual = actual.map(|_| fake_memory_version("v1"));
    (!condition.is_satisfied_by(actual.as_ref())).then(|| MemoryConflict {
        expected: condition.expected_version().cloned(),
        actual,
        current_value: None,
    })
}

fn memory_prefix(region: &str, path: &[&str]) -> String {
    format!("{region}:{}:", path.join("."))
}

fn matches_predicate(key: &HostValue, value: &HostValue, predicate: Option<&HostValue>) -> bool {
    let Some(predicate) = predicate else {
        return true;
    };
    match predicate {
        HostValue::String(text) => {
            matches!(key, HostValue::String(key_text) if key_text.contains(text))
                || matches!(value, HostValue::String(value_text) if value_text.contains(text))
        }
        other => key == other || value == other,
    }
}

fn extract_embedding(value: &HostValue) -> Option<&[HostValue]> {
    match value {
        HostValue::List(values) => Some(values),
        HostValue::Record(fields) => fields.iter().find_map(|(name, value)| {
            (name == "embedding").then_some(value).and_then(|value| {
                if let HostValue::List(values) = value {
                    Some(values.as_slice())
                } else {
                    None
                }
            })
        }),
        _ => None,
    }
}

fn cosine_similarity(query: &[f32], candidate: &[HostValue]) -> Option<f32> {
    if query.len() != candidate.len() || query.is_empty() {
        return None;
    }
    let mut dot = 0.0f32;
    let mut query_norm = 0.0f32;
    let mut candidate_norm = 0.0f32;
    for (left, right) in query.iter().copied().zip(candidate.iter()) {
        let right = host_value_to_f32(right)?;
        dot += left * right;
        query_norm += left * left;
        candidate_norm += right * right;
    }
    if query_norm == 0.0 || candidate_norm == 0.0 {
        return None;
    }
    Some(dot / (query_norm.sqrt() * candidate_norm.sqrt()))
}

fn host_value_to_f32(value: &HostValue) -> Option<f32> {
    match value {
        HostValue::Float(value) if value.is_finite() => Some(*value as f32),
        HostValue::Int(value) => Some(*value as f32),
        HostValue::UInt(value) => Some(*value as f32),
        _ => None,
    }
}
