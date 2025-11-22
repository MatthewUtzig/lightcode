#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use code_app_server_protocol::AuthMode;
use code_auto_drive_core::{
    build_initial_planning_seed,
    filter_popular_commands,
    AutoContinueMode, AutoControllerEffect, AutoDriveController, AutoRunPhase, AutoTurnAgentsTiming,
};
use code_core::agent_defaults::model_guide_markdown_with_custom;
use code_core::coalesce_snapshot_records;
use code_core::config::{Config, ConfigOverrides};
use code_core::debug_logger::DebugLogger;
use code_core::fork_history_from_response_items;
use code_core::models::{ContentItem, ResponseItem};
use code_core::prune_history_after_dropping_last_user_turns;
use code_core::retain_api_messages_only;
use code_core::summarize_snapshot;
use code_core::token_data::parse_id_token;
use code_core::AuthManager;
use code_core::ModelClient;
use code_core::Prompt;
use code_core::ResponseEvent;
use code_core::ResponseStream;
use code_core::SnapshotRecordPayload;
use code_core::protocol::TokenUsage;
use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;
use futures::StreamExt;
use once_cell::sync::{Lazy, OnceCell};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::runtime::Builder as TokioRuntimeBuilder;
use uuid::Uuid;

static CONFIG: Lazy<Mutex<Option<Value>>> = Lazy::new(|| Mutex::new(None));
static KOTLIN_CONFIG: OnceCell<Arc<Config>> = OnceCell::new();
const SIMPLE_MODEL_FIXTURE_ENV: &str = "CODE_KOTLIN_SIMPLE_MODEL_FIXTURE";

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ExecuteRequest {
    Echo { payload: Value },
    ParseIdToken { token: String },
    AutoDriveCountdownTick(AutoDriveCountdownTickRequest),
    AutoDriveUpdateContinueMode(AutoDriveUpdateContinueModeRequest),
    AutoDriveSequence(AutoDriveSequenceRequest),
    ConversationPruneHistory(ConversationPruneHistoryRequest),
    ConversationFilterHistory(ConversationFilterHistoryRequest),
    ConversationCoalesceSnapshot(ConversationCoalesceSnapshotRequest),
    ConversationSnapshotSummary(ConversationSnapshotSummaryRequest),
    ConversationForkHistory(ConversationForkHistoryRequest),
    ConversationFilterPopularCommands(ConversationFilterPopularCommandsRequest),
    AutoCoordinatorPlanningSeed(PlannerSeedRequest),
    SimpleModelTurn(SimpleModelTurnRequest),
}

impl From<PhaseInput> for AutoRunPhase {
    fn from(input: PhaseInput) -> Self {
        match input {
            PhaseInput::Idle => AutoRunPhase::Idle,
            PhaseInput::AwaitingGoalEntry => AutoRunPhase::AwaitingGoalEntry,
            PhaseInput::Launching => AutoRunPhase::Launching,
            PhaseInput::Active => AutoRunPhase::Active,
            PhaseInput::PausedManual {
                resume_after_submit,
                bypass_next_submit,
            } => AutoRunPhase::PausedManual {
                resume_after_submit,
                bypass_next_submit,
            },
            PhaseInput::AwaitingCoordinator { prompt_ready } => {
                AutoRunPhase::AwaitingCoordinator { prompt_ready }
            }
            PhaseInput::AwaitingDiagnostics { coordinator_waiting } => {
                AutoRunPhase::AwaitingDiagnostics { coordinator_waiting }
            }
            PhaseInput::AwaitingReview { diagnostics_pending } => {
                AutoRunPhase::AwaitingReview { diagnostics_pending }
            }
            PhaseInput::TransientRecovery { backoff_ms } => {
                AutoRunPhase::TransientRecovery { backoff_ms }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct AutoDriveCountdownTickRequest {
    phase: PhaseInput,
    countdown_id: u64,
    decision_seq: u64,
    seconds_left: u8,
}

#[derive(Debug, Deserialize)]
struct AutoDriveUpdateContinueModeRequest {
    phase: PhaseInput,
    continue_mode: ContinueModeInput,
    countdown_id: u64,
    decision_seq: u64,
}

#[derive(Debug, Deserialize)]
struct AutoDriveSequenceRequest {
    initial_state: ControllerStateInput,
    operations: Vec<ControllerOperationInput>,
}

#[derive(Debug, Deserialize)]
struct ConversationPruneHistoryRequest {
    history: Vec<ResponseItem>,
    drop_last_user_turns: u32,
}

#[derive(Debug, Deserialize)]
struct ConversationFilterHistoryRequest {
    history: Vec<ResponseItem>,
}

#[derive(Debug, Deserialize)]
struct ConversationCoalesceSnapshotRequest {
    records: Vec<SnapshotRecordPayload>,
}

#[derive(Debug, Deserialize)]
struct ConversationSnapshotSummaryRequest {
    records: Vec<SnapshotRecordPayload>,
}

#[derive(Debug, Deserialize)]
struct ConversationForkHistoryRequest {
    history: Vec<ResponseItem>,
    drop_last_user_turns: u32,
}

#[derive(Debug, Deserialize)]
struct ConversationFilterPopularCommandsRequest {
    history: Vec<ResponseItem>,
}

#[derive(Debug, Deserialize)]
struct PlannerSeedRequest {
    goal_text: String,
    include_agents: bool,
}

#[derive(Debug, Deserialize)]
struct SimpleModelTurnRequest {
    history: Vec<Value>,
    #[serde(rename = "latest_user_prompt")]
    latest_user_prompt: Option<String>,
}

struct SimpleModelTurnResult {
    thinking: Vec<String>,
    answer: String,
    token_usage: Option<TokenUsage>,
}

#[derive(Debug, Deserialize)]
struct SimpleModelTurnFixture {
    thinking: Vec<String>,
    answer: String,
}

#[derive(Debug, Deserialize)]
struct ControllerStateInput {
    phase: PhaseInput,
    continue_mode: ContinueModeInput,
    countdown_id: u64,
    countdown_decision_seq: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControllerOperationInput {
    UpdateContinueMode { mode: ContinueModeInput },
    HandleCountdownTick {
        countdown_id: u64,
        decision_seq: u64,
        seconds_left: u8,
    },
    PauseForTransientFailure { reason: String },
    StopRun { message: Option<String> },
    LaunchResult {
        result: LaunchOutcomeInput,
        goal: String,
        error: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchOutcomeInput {
    Succeeded,
    Failed,
}

#[derive(Debug, Serialize)]
struct SequenceStep {
    effects: Vec<Value>,
    snapshot: ControllerSnapshot,
}

#[derive(Debug, Serialize)]
struct ControllerSnapshot {
    phase: PhaseInput,
    continue_mode: ContinueModeInput,
    countdown_id: u64,
    countdown_decision_seq: u64,
    seconds_remaining: u8,
    transient_restart_attempts: u32,
    restart_token: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "name", rename_all = "snake_case")]
enum PhaseInput {
    Idle,
    AwaitingGoalEntry,
    Launching,
    Active,
    PausedManual {
        resume_after_submit: bool,
        bypass_next_submit: bool,
    },
    AwaitingCoordinator {
        prompt_ready: bool,
    },
    AwaitingDiagnostics {
        coordinator_waiting: bool,
    },
    AwaitingReview {
        diagnostics_pending: bool,
    },
    TransientRecovery {
        backoff_ms: u64,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ContinueModeInput {
    Immediate,
    TenSeconds,
    SixtySeconds,
    Manual,
}

impl From<&AutoRunPhase> for PhaseInput {
    fn from(phase: &AutoRunPhase) -> Self {
        match phase {
            AutoRunPhase::Idle => PhaseInput::Idle,
            AutoRunPhase::AwaitingGoalEntry => PhaseInput::AwaitingGoalEntry,
            AutoRunPhase::Launching => PhaseInput::Launching,
            AutoRunPhase::Active => PhaseInput::Active,
            AutoRunPhase::PausedManual {
                resume_after_submit,
                bypass_next_submit,
            } => PhaseInput::PausedManual {
                resume_after_submit: *resume_after_submit,
                bypass_next_submit: *bypass_next_submit,
            },
            AutoRunPhase::AwaitingCoordinator { prompt_ready } => PhaseInput::AwaitingCoordinator {
                prompt_ready: *prompt_ready,
            },
            AutoRunPhase::AwaitingDiagnostics { coordinator_waiting } => {
                PhaseInput::AwaitingDiagnostics {
                    coordinator_waiting: *coordinator_waiting,
                }
            }
            AutoRunPhase::AwaitingReview { diagnostics_pending } => PhaseInput::AwaitingReview {
                diagnostics_pending: *diagnostics_pending,
            },
            AutoRunPhase::TransientRecovery { backoff_ms } => {
                PhaseInput::TransientRecovery { backoff_ms: *backoff_ms }
            }
        }
    }
}

impl From<AutoContinueMode> for ContinueModeInput {
    fn from(mode: AutoContinueMode) -> Self {
        match mode {
            AutoContinueMode::Immediate => ContinueModeInput::Immediate,
            AutoContinueMode::TenSeconds => ContinueModeInput::TenSeconds,
            AutoContinueMode::SixtySeconds => ContinueModeInput::SixtySeconds,
            AutoContinueMode::Manual => ContinueModeInput::Manual,
        }
    }
}

impl From<ContinueModeInput> for AutoContinueMode {
    fn from(input: ContinueModeInput) -> Self {
        match input {
            ContinueModeInput::Immediate => AutoContinueMode::Immediate,
            ContinueModeInput::TenSeconds => AutoContinueMode::TenSeconds,
            ContinueModeInput::SixtySeconds => AutoContinueMode::SixtySeconds,
            ContinueModeInput::Manual => AutoContinueMode::Manual,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_lightcode_core_jni_RustCoreBridge_initialize(
    mut env: JNIEnv,
    _class: JClass,
    config_json: JString,
) {
    if let Err(err) = initialize_impl(&mut env, config_json) {
        let _ = env.throw_new("java/lang/RuntimeException", err);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_lightcode_core_jni_RustCoreBridge_shutdown(
    mut env: JNIEnv,
    _class: JClass,
) {
    if let Err(err) = shutdown_impl() {
        let _ = env.throw_new("java/lang/RuntimeException", err);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_lightcode_core_jni_RustCoreBridge_execute(
    mut env: JNIEnv,
    _class: JClass,
    request_json: JString,
) -> jstring {
    match execute_impl(&mut env, request_json) {
        Ok(result) => result,
        Err(err) => {
            let fallback = json!({
                "status": "error",
                "message": err,
            });
            env.new_string(fallback.to_string())
                .map(|s| s.into_raw())
                .unwrap_or(std::ptr::null_mut())
        }
    }
}

fn initialize_impl(env: &mut JNIEnv, config_json: JString) -> Result<(), String> {
    let config = get_string(env, config_json)?;
    let parsed: Value = serde_json::from_str(&config).map_err(|e| e.to_string())?;
    let mut guard = CONFIG.lock().map_err(|_| "config mutex poisoned".to_string())?;
    *guard = Some(parsed);
    Ok(())
}

fn shutdown_impl() -> Result<(), String> {
    let mut guard = CONFIG.lock().map_err(|_| "config mutex poisoned".to_string())?;
    *guard = None;
    Ok(())
}

fn execute_impl(env: &mut JNIEnv, request_json: JString) -> Result<jstring, String> {
    let request_str = get_string(env, request_json)?;
    let req: ExecuteRequest = serde_json::from_str(&request_str)
        .map_err(|e| format!("{} in payload {}", e, request_str))?;
    let response = handle_request(req);
    let response_str = serde_json::to_string(&response).map_err(|e| e.to_string())?;
    let output = env
        .new_string(response_str)
        .map_err(|e| e.to_string())?
        .into_raw();
    Ok(output)
}

fn handle_request(request: ExecuteRequest) -> Value {
    match request {
        ExecuteRequest::Echo { payload } => json!({
            "status": "ok",
            "kind": "echo",
            "payload": payload,
        }),
        ExecuteRequest::ParseIdToken { token } => match parse_id_token(&token) {
            Ok(info) => {
                let email = info.email.clone();
                let plan = info.get_chatgpt_plan_type();
                json!({
                    "status": "ok",
                    "kind": "parsed_id_token",
                    "email": email,
                    "chatgpt_plan_type": plan,
                })
            }
            Err(err) => json!({
                "status": "error",
                "message": err.to_string(),
            }),
        },
        ExecuteRequest::AutoDriveCountdownTick(req) => {
            handle_auto_drive_countdown_tick(req)
        }
        ExecuteRequest::AutoDriveUpdateContinueMode(req) => {
            handle_auto_drive_update_continue_mode(req)
        }
        ExecuteRequest::AutoDriveSequence(req) => handle_auto_drive_sequence(req),
        ExecuteRequest::ConversationPruneHistory(req) => {
            handle_conversation_prune_history(req)
        }
        ExecuteRequest::ConversationFilterHistory(req) => {
            handle_conversation_filter_history(req)
        }
        ExecuteRequest::ConversationCoalesceSnapshot(req) => {
            handle_conversation_coalesce_snapshot(req)
        }
        ExecuteRequest::ConversationSnapshotSummary(req) => {
            handle_conversation_snapshot_summary(req)
        }
        ExecuteRequest::ConversationForkHistory(req) => {
            handle_conversation_fork_history(req)
        }
        ExecuteRequest::ConversationFilterPopularCommands(req) => {
            handle_conversation_filter_popular_commands(req)
        }
        ExecuteRequest::AutoCoordinatorPlanningSeed(req) => {
            handle_planner_seed_request(req)
        }
        ExecuteRequest::SimpleModelTurn(req) => handle_simple_model_turn(req),
    }
}

fn handle_auto_drive_countdown_tick(req: AutoDriveCountdownTickRequest) -> Value {
    let mut controller = AutoDriveController::default();
    controller.phase = req.phase.into();
    controller.countdown_id = req.countdown_id;
    controller.countdown_decision_seq = req.decision_seq;

    let effects = controller.handle_countdown_tick(
        req.countdown_id,
        req.decision_seq,
        req.seconds_left,
    );

    json!({
        "status": "ok",
        "kind": "auto_drive_countdown_tick",
        "effects": effects.iter().map(effect_to_json).collect::<Vec<_>>(),
        "seconds_left": controller.seconds_remaining,
    })
}

fn handle_auto_drive_update_continue_mode(req: AutoDriveUpdateContinueModeRequest) -> Value {
    let mut controller = AutoDriveController::default();
    controller.phase = req.phase.into();
    controller.countdown_id = req.countdown_id;
    controller.countdown_decision_seq = req.decision_seq;

    let effects = controller.update_continue_mode(req.continue_mode.into());

    json!({
        "status": "ok",
        "kind": "auto_drive_update_continue_mode",
        "effects": effects.iter().map(effect_to_json).collect::<Vec<_>>(),
        "seconds_left": controller.seconds_remaining,
    })
}

impl From<&AutoDriveController> for ControllerSnapshot {
    fn from(controller: &AutoDriveController) -> Self {
        ControllerSnapshot {
            phase: PhaseInput::from(controller.phase()),
            continue_mode: ContinueModeInput::from(controller.continue_mode),
            countdown_id: controller.countdown_id,
            countdown_decision_seq: controller.countdown_decision_seq,
            seconds_remaining: controller.seconds_remaining,
            transient_restart_attempts: controller.transient_restart_attempts,
            restart_token: controller.restart_token,
        }
    }
}

fn handle_auto_drive_sequence(req: AutoDriveSequenceRequest) -> Value {
    let mut controller = AutoDriveController::default();
    controller.phase = req.initial_state.phase.clone().into();
    controller.continue_mode = req.initial_state.continue_mode.into();
    controller.countdown_id = req.initial_state.countdown_id;
    controller.countdown_decision_seq = req.initial_state.countdown_decision_seq;
    controller.seconds_remaining = controller.countdown_seconds().unwrap_or(0);

    let mut steps = Vec::with_capacity(req.operations.len());
    for op in req.operations {
        let effects = match op {
            ControllerOperationInput::UpdateContinueMode { mode } => {
                controller.update_continue_mode(mode.into())
            }
            ControllerOperationInput::HandleCountdownTick {
                countdown_id,
                decision_seq,
                seconds_left,
            } => controller.handle_countdown_tick(countdown_id, decision_seq, seconds_left),
            ControllerOperationInput::PauseForTransientFailure { reason } => {
                controller.pause_for_transient_failure(Instant::now(), reason)
            }
            ControllerOperationInput::StopRun { message } => {
                controller.stop_run(Instant::now(), message)
            }
            ControllerOperationInput::LaunchResult { result, goal, error } => match result {
                LaunchOutcomeInput::Succeeded => controller.launch_succeeded(goal, None, Instant::now()),
                LaunchOutcomeInput::Failed => {
                    controller.launch_failed(goal, error.unwrap_or_else(|| "unknown error".to_string()))
                }
            },
        };

        let snapshot = ControllerSnapshot::from(&controller);
        let serialized_effects: Vec<Value> = effects.iter().map(effect_to_json).collect();
        steps.push(SequenceStep {
            effects: serialized_effects,
            snapshot,
        });
    }

    json!({
        "status": "ok",
        "kind": "auto_drive_sequence",
        "steps": steps,
    })
}

fn handle_conversation_prune_history(req: ConversationPruneHistoryRequest) -> Value {
    let outcome = prune_history_after_dropping_last_user_turns(
        req.history,
        req.drop_last_user_turns as usize,
    );

    json!({
        "status": "ok",
        "kind": "conversation_prune_history",
        "history": outcome.retained_history,
        "pruned_user_turns": outcome.pruned_user_turns,
        "was_reset": outcome.was_reset,
    })
}

fn handle_conversation_filter_history(req: ConversationFilterHistoryRequest) -> Value {
    let outcome = retain_api_messages_only(req.history);

    json!({
        "status": "ok",
        "kind": "conversation_filter_history",
        "history": outcome.history,
        "removed_count": outcome.removed_count,
    })
}

fn handle_conversation_coalesce_snapshot(req: ConversationCoalesceSnapshotRequest) -> Value {
    let outcome = coalesce_snapshot_records(req.records);

    json!({
        "status": "ok",
        "kind": "conversation_coalesce_snapshot",
        "records": outcome.records,
        "removed_count": outcome.removed_count,
    })
}

fn handle_conversation_snapshot_summary(req: ConversationSnapshotSummaryRequest) -> Value {
    let summary = summarize_snapshot(req.records);

    json!({
        "status": "ok",
        "kind": "conversation_snapshot_summary",
        "record_count": summary.record_count,
        "assistant_messages": summary.assistant_messages,
        "user_messages": summary.user_messages,
    })
}

fn handle_conversation_fork_history(req: ConversationForkHistoryRequest) -> Value {
    let outcome = fork_history_from_response_items(req.history, req.drop_last_user_turns as usize);

    json!({
        "status": "ok",
        "kind": "conversation_fork_history",
        "history": outcome.retained_history,
        "dropped_user_turns": outcome.dropped_user_turns,
        "became_new": outcome.became_new,
    })
}

fn handle_conversation_filter_popular_commands(req: ConversationFilterPopularCommandsRequest) -> Value {
    let filtered = filter_popular_commands(req.history);
    json!({
        "status": "ok",
        "kind": "conversation_filter_popular_commands",
        "history": filtered,
    })
}

fn handle_planner_seed_request(req: PlannerSeedRequest) -> Value {
    let seed = build_initial_planning_seed(&req.goal_text, req.include_agents);
    match seed {
        Some(seed) => {
            let agents_timing = seed.agents_timing.map(|timing| match timing {
                AutoTurnAgentsTiming::Parallel => "Parallel",
                AutoTurnAgentsTiming::Blocking => "Blocking",
            });

            json!({
                "status": "ok",
                "kind": "auto_coordinator_planning_seed",
                "response_json": seed.response_json,
                "cli_prompt": seed.cli_prompt,
                "goal_message": seed.goal_message,
                "status_title": seed.status_title,
                "status_sent_to_user": seed.status_sent_to_user,
                "agents_timing": agents_timing,
            })
        }
        None => json!({
            "status": "ok",
            "kind": "auto_coordinator_planning_seed",
            "response_json": null,
        }),
    }
}

fn handle_simple_model_turn(req: SimpleModelTurnRequest) -> Value {
    if let Some(path) = std::env::var_os(SIMPLE_MODEL_FIXTURE_ENV) {
        let fixture_path = PathBuf::from(path);
        match load_simple_model_fixture(&fixture_path) {
            Ok(result) => {
                return json!({
                    "status": "ok",
                    "kind": "simple_model_turn",
                    "thinking": result.thinking,
                    "answer": result.answer,
                    "token_usage": result.token_usage,
                });
            }
            Err(err) => {
                return json!({
                    "status": "error",
                    "kind": "simple_model_turn",
                    "message": format!("fixture_error: {err}"),
                });
            }
        }
    }

    match run_simple_model_turn(req) {
        Ok(result) => json!({
            "status": "ok",
            "kind": "simple_model_turn",
            "thinking": result.thinking,
            "answer": result.answer,
            "token_usage": result.token_usage,
        }),
        Err(err) => json!({
            "status": "error",
            "kind": "simple_model_turn",
            "message": err,
        }),
    }
}

fn run_simple_model_turn(req: SimpleModelTurnRequest) -> Result<SimpleModelTurnResult, String> {
    let config = load_kotlin_config()?;

    let prompt_text = req
        .latest_user_prompt
        .or_else(|| latest_user_prompt_from_history(&req.history))
        .ok_or_else(|| "latest_user_prompt_required".to_string())?;

    let prompt = build_simple_prompt(&config, prompt_text.clone());
    let runtime = TokioRuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;

    runtime.block_on(async move {
        let client = build_model_client(config.clone())?;
        let stream = client.stream(&prompt).await.map_err(|err| err.to_string())?;
        collect_simple_model_stream(stream).await
    })
}

fn load_kotlin_config() -> Result<Arc<Config>, String> {
    KOTLIN_CONFIG
        .get_or_try_init(|| {
            Config::load_with_cli_overrides(Vec::new(), ConfigOverrides::default())
                .map(Arc::new)
                .map_err(|err| err.to_string())
        })
        .map(|cfg| Arc::clone(cfg))
}

fn build_model_client(config: Arc<Config>) -> Result<ModelClient, String> {
    let preferred_auth = if config.using_chatgpt_auth {
        AuthMode::ChatGPT
    } else {
        AuthMode::ApiKey
    };
    let auth_manager = AuthManager::shared_with_mode_and_originator(
        config.code_home.clone(),
        preferred_auth,
        config.responses_originator_header.clone(),
    );
    let logger = DebugLogger::new(config.debug)
        .or_else(|_| DebugLogger::new(false))
        .map_err(|err| err.to_string())?;

    Ok(ModelClient::new(
        config.clone(),
        Some(auth_manager),
        None,
        config.model_provider.clone(),
        config.model_reasoning_effort,
        config.model_reasoning_summary,
        config.model_text_verbosity,
        Uuid::new_v4(),
        Arc::new(Mutex::new(logger)),
    ))
}

fn build_simple_prompt(
    config: &Arc<Config>,
    latest_user_prompt: String,
) -> Prompt {
    let mut prompt = Prompt::default();
    prompt.input = vec![ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: latest_user_prompt,
        }],
    }];
    prompt.store = !config.disable_response_storage;
    prompt.user_instructions = config.user_instructions.clone();
    prompt.base_instructions_override = config.base_instructions.clone();
    prompt.include_additional_instructions = true;
    prompt.model_override = Some(config.model.clone());
    prompt.model_family_override = Some(config.model_family.clone());
    prompt.model_descriptions = model_guide_markdown_with_custom(&config.agents);

    prompt
}

fn latest_user_prompt_from_history(history: &[Value]) -> Option<String> {
    history.iter().rev().find_map(|entry| {
        let obj = entry.as_object()?;
        let item_type = obj.get("type")?.as_str()?;
        let role = obj.get("role")?.as_str()?;
        if item_type != "message" || role != "user" {
            return None;
        }
        let content = obj.get("content")?.as_array()?;
        content.iter().rev().find_map(|piece| {
            let piece_obj = piece.as_object()?;
            if piece_obj.get("type")?.as_str()? != "input_text" {
                return None;
            }
            piece_obj.get("text")?.as_str().map(|text| text.to_string())
        })
    })
}

async fn collect_simple_model_stream(
    mut stream: ResponseStream,
) -> Result<SimpleModelTurnResult, String> {
    let mut thinking_chunks: Vec<String> = Vec::new();
    let mut current_thinking = String::new();
    let mut answer_chunks: Vec<String> = Vec::new();

    let mut token_usage: Option<TokenUsage> = None;

    while let Some(event) = stream.next().await {
        let event = event.map_err(|err| err.to_string())?;
        match event {
            ResponseEvent::ReasoningSummaryDelta { delta, .. }
            | ResponseEvent::ReasoningContentDelta { delta, .. } => {
                current_thinking.push_str(&delta);
            }
            ResponseEvent::ReasoningSummaryPartAdded => {
                if !current_thinking.trim().is_empty() {
                    thinking_chunks.push(current_thinking.trim().to_string());
                }
                current_thinking.clear();
            }
            ResponseEvent::OutputTextDelta { delta, .. } => {
                answer_chunks.push(delta);
            }
            ResponseEvent::OutputItemDone { item, .. } => {
                if let ResponseItem::Message { content, .. } = item {
                    for piece in content {
                        if let ContentItem::OutputText { text } = piece {
                            answer_chunks.push(text);
                        }
                    }
                }
            }
            ResponseEvent::Completed { token_usage: usage, .. } => {
                token_usage = usage;
                break;
            }
            _ => {}
        }
    }

    if !current_thinking.trim().is_empty() {
        thinking_chunks.push(current_thinking.trim().to_string());
    }

    let answer = answer_chunks.join("").trim().to_string();
    if answer.is_empty() {
        return Err("model_returned_empty_answer".to_string());
    }

    Ok(SimpleModelTurnResult {
        thinking: thinking_chunks,
        answer,
        token_usage,
    })
}

fn load_simple_model_fixture(path: &Path) -> Result<SimpleModelTurnResult, String> {
    let contents = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let fixture: SimpleModelTurnFixture = serde_json::from_str(&contents).map_err(|err| err.to_string())?;
    Ok(SimpleModelTurnResult {
        thinking: fixture
            .thinking
            .into_iter()
            .filter(|chunk| !chunk.trim().is_empty())
            .collect(),
        answer: fixture.answer,
        token_usage: None,
    })
}

fn effect_to_json(effect: &AutoControllerEffect) -> Value {
    match effect {
        AutoControllerEffect::RefreshUi => json!({"type": "refresh_ui"}),
        AutoControllerEffect::SubmitPrompt => json!({"type": "submit_prompt"}),
        AutoControllerEffect::StartCountdown {
            countdown_id,
            decision_seq,
            seconds,
        } => json!({
            "type": "start_countdown",
            "countdown_id": countdown_id,
            "decision_seq": decision_seq,
            "seconds": seconds,
        }),
        AutoControllerEffect::LaunchStarted { goal } => json!({
            "type": "launch_started",
            "message": goal,
        }),
        AutoControllerEffect::LaunchFailed { goal, error } => json!({
            "type": "launch_failed",
            "message": goal,
            "hint": error,
        }),
        AutoControllerEffect::ShowGoalEntry => json!({"type": "show_goal_entry"}),
        AutoControllerEffect::CancelCoordinator => json!({"type": "cancel_coordinator"}),
        AutoControllerEffect::SetTaskRunning { running } => {
            json!({"type": "set_task_running", "running": running})
        }
        AutoControllerEffect::UpdateTerminalHint { hint } => json!({
            "type": "update_terminal_hint",
            "hint": hint,
        }),
        AutoControllerEffect::TransientPause {
            attempt,
            delay,
            reason,
        } => json!({
            "type": "transient_pause",
            "attempt": attempt,
            "delay_ms": delay.as_millis() as u64,
            "reason": reason,
        }),
        AutoControllerEffect::ScheduleRestart {
            token,
            attempt,
            delay,
        } => json!({
            "type": "schedule_restart",
            "token": token,
            "attempt": attempt,
            "delay_ms": delay.as_millis() as u64,
        }),
        AutoControllerEffect::ClearCoordinatorView => {
            json!({"type": "clear_coordinator_view"})
        }
        AutoControllerEffect::ResetHistory => json!({"type": "reset_history"}),
        AutoControllerEffect::EnsureInputFocus => json!({"type": "ensure_input_focus"}),
        AutoControllerEffect::StopCompleted { summary, message } => json!({
            "type": "stop_completed",
            "turns_completed": summary.turns_completed,
            "duration_ms": summary.duration.as_millis() as u64,
            "message": message,
        }),
    }
}

fn get_string(env: &mut JNIEnv, input: JString) -> Result<String, String> {
    env.get_string(&input)
        .map_err(|e| e.to_string())
        .map(|s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::{handle_request, ExecuteRequest};
    use serde_json::json;

    #[test]
    fn countdown_tick_refreshes_when_time_remaining() {
        let req_json = json!({
            "type": "auto_drive_countdown_tick",
            "phase": { "name": "awaiting_coordinator", "prompt_ready": true },
            "countdown_id": 7,
            "decision_seq": 3,
            "seconds_left": 5
        });
        let request: ExecuteRequest = serde_json::from_value(req_json).expect("request to parse");

        let response = handle_request(request);

        assert_eq!(response["status"], "ok");
        assert_eq!(response["kind"], "auto_drive_countdown_tick");
        assert_eq!(response["seconds_left"], 5);
        assert_eq!(response["effects"].as_array().unwrap().len(), 1);
        assert_eq!(response["effects"][0]["type"], "refresh_ui");
    }

    #[test]
    fn countdown_tick_submits_when_timer_hits_zero() {
        let req_json = json!({
            "type": "auto_drive_countdown_tick",
            "phase": { "name": "awaiting_coordinator", "prompt_ready": true },
            "countdown_id": 42,
            "decision_seq": 9,
            "seconds_left": 0
        });
        let request: ExecuteRequest = serde_json::from_value(req_json).expect("request to parse");

        let response = handle_request(request);

        assert_eq!(response["status"], "ok");
        assert_eq!(response["kind"], "auto_drive_countdown_tick");
        assert_eq!(response["seconds_left"], 0);
        assert_eq!(response["effects"].as_array().unwrap().len(), 1);
        assert_eq!(response["effects"][0]["type"], "submit_prompt");
    }

    #[test]
    fn countdown_tick_ignored_when_phase_not_waiting() {
        let req_json = json!({
            "type": "auto_drive_countdown_tick",
            "phase": { "name": "active" },
            "countdown_id": 1,
            "decision_seq": 1,
            "seconds_left": 4
        });
        let request: ExecuteRequest = serde_json::from_value(req_json).expect("request to parse");

        let response = handle_request(request);

        assert_eq!(response["status"], "ok");
        assert_eq!(response["effects"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn update_continue_mode_triggers_countdown_when_waiting() {
        let req_json = json!({
            "type": "auto_drive_update_continue_mode",
            "phase": { "name": "awaiting_coordinator", "prompt_ready": true },
            "continue_mode": "ten_seconds",
            "countdown_id": 8,
            "decision_seq": 11
        });
        let request: ExecuteRequest = serde_json::from_value(req_json).expect("request to parse");

        let response = handle_request(request);

        assert_eq!(response["status"], "ok");
        assert_eq!(response["kind"], "auto_drive_update_continue_mode");
        assert_eq!(response["seconds_left"], 10);
        let effects = response["effects"].as_array().unwrap();
        assert!(effects.iter().any(|eff| eff["type"] == "start_countdown"));
    }

    #[test]
    fn update_continue_mode_only_refreshes_when_not_waiting() {
        let req_json = json!({
            "type": "auto_drive_update_continue_mode",
            "phase": { "name": "active" },
            "continue_mode": "manual",
            "countdown_id": 1,
            "decision_seq": 2
        });
        let request: ExecuteRequest = serde_json::from_value(req_json).expect("request to parse");

        let response = handle_request(request);

        assert_eq!(response["status"], "ok");
        assert_eq!(response["seconds_left"], 0);
        let effects = response["effects"].as_array().unwrap();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0]["type"], "refresh_ui");
    }

    #[test]
    fn sequence_request_tracks_snapshots() {
        let req_json = json!({
            "type": "auto_drive_sequence",
            "initial_state": {
                "phase": { "name": "awaiting_coordinator", "prompt_ready": true },
                "continue_mode": "ten_seconds",
                "countdown_id": 10,
                "countdown_decision_seq": 3
            },
            "operations": [
                { "type": "update_continue_mode", "mode": "sixty_seconds" },
                { "type": "handle_countdown_tick", "countdown_id": 11, "decision_seq": 3, "seconds_left": 0 },
                { "type": "pause_for_transient_failure", "reason": "network" }
            ]
        });
        let request: ExecuteRequest = serde_json::from_value(req_json).expect("request to parse");

        let response = handle_request(request);
        assert_eq!(response["status"], "ok");
        assert_eq!(response["kind"], "auto_drive_sequence");
        let steps = response["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0]["effects"].as_array().unwrap()[0]["type"], "refresh_ui");
        assert_eq!(steps[1]["effects"].as_array().unwrap()[0]["type"], "submit_prompt");
        assert_eq!(steps[2]["effects"].as_array().unwrap()[0]["type"], "cancel_coordinator");
        assert_eq!(steps[2]["snapshot"]["phase"]["name"], "transient_recovery");
    }
}
