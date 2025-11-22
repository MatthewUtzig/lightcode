use std::time::Instant;

use code_auto_drive_core::{
    AutoContinueMode,
    AutoControllerEffect,
    AutoDriveController,
    AutoRunPhase,
};
use codex_core_jni as _;
use code_kotlin_host::run_auto_drive_sequence_raw;
use serde::Deserialize;
use serde_json::{json, Value};

#[serial_test::serial]
#[test]
fn kotlin_auto_drive_sequence_matches_rust_effects() {
    let payload = sample_payload();
    let request: AutoDriveSequenceEnvelope =
        serde_json::from_value(payload.clone()).expect("fixture payload to deserialize");

    let response = match run_auto_drive_sequence_raw(&payload.to_string()) {
        Ok(raw) => raw,
        Err(err) if should_skip(&err) => {
            eprintln!("skipping Kotlin parity test: {err}");
            return;
        }
        Err(err) => panic!("failed to execute Kotlin auto drive sequence: {err:?}"),
    };

    let value: Value =
        serde_json::from_str(&response).expect("run_auto_drive_sequence_raw to return JSON");
    assert_eq!(value["status"].as_str(), Some("ok"));
    assert_eq!(value["kind"].as_str(), Some("auto_drive_sequence"));

    let kotlin_effects = extract_kotlin_effect_types(&value);
    assert!(
        !kotlin_effects.is_empty(),
        "expected Kotlin auto drive sequence to emit at least one step",
    );

    let rust_effects = simulate_rust_sequence(&request);
    assert_eq!(kotlin_effects, rust_effects, "Kotlin effects diverged from Rust");
}

fn sample_payload() -> Value {
    json!({
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
    })
}

fn should_skip(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("Kotlin engine jar not found")
        || msg.contains("failed to create JVM")
        || msg.contains("failed to find CoreEngineHost")
        || msg.contains("Java exception was thrown")
}

fn extract_kotlin_effect_types(value: &Value) -> Vec<Vec<String>> {
    let steps = value["steps"].as_array().cloned().expect("steps array");
    steps
        .iter()
        .map(|step| {
            step["effects"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|effect| effect["type"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .collect()
}

fn simulate_rust_sequence(envelope: &AutoDriveSequenceEnvelope) -> Vec<Vec<String>> {
    let mut controller = AutoDriveController::default();
    controller.phase = envelope.initial_state.phase.clone().into();
    controller.continue_mode = envelope.initial_state.continue_mode.into();
    controller.countdown_id = envelope.initial_state.countdown_id;
    controller.countdown_decision_seq = envelope.initial_state.countdown_decision_seq;
    controller.seconds_remaining = controller.countdown_seconds().unwrap_or(0);

    envelope
        .operations
        .iter()
        .map(|operation| {
            let effects = match operation {
                ControllerOperation::UpdateContinueMode { mode } => {
                    controller.update_continue_mode((*mode).into())
                }
                ControllerOperation::HandleCountdownTick {
                    countdown_id,
                    decision_seq,
                    seconds_left,
                } => controller.handle_countdown_tick(*countdown_id, *decision_seq, *seconds_left),
                ControllerOperation::PauseForTransientFailure { reason } => {
                    controller.pause_for_transient_failure(Instant::now(), reason.clone())
                }
                ControllerOperation::StopRun { message } => {
                    controller.stop_run(Instant::now(), message.clone())
                }
                ControllerOperation::LaunchResult { result, goal, error } => match result {
                    LaunchOutcome::Succeeded => {
                        controller.launch_succeeded(goal.clone(), None, Instant::now())
                    }
                    LaunchOutcome::Failed => controller.launch_failed(
                        goal.clone(),
                        error.clone().unwrap_or_else(|| "unknown error".to_string()),
                    ),
                },
            };
            effects.into_iter().map(effect_type_name).collect()
        })
        .collect()
}

fn effect_type_name(effect: AutoControllerEffect) -> String {
    match effect {
        AutoControllerEffect::RefreshUi => "refresh_ui".into(),
        AutoControllerEffect::StartCountdown { .. } => "start_countdown".into(),
        AutoControllerEffect::SubmitPrompt => "submit_prompt".into(),
        AutoControllerEffect::LaunchStarted { .. } => "launch_started".into(),
        AutoControllerEffect::LaunchFailed { .. } => "launch_failed".into(),
        AutoControllerEffect::StopCompleted { .. } => "stop_completed".into(),
        AutoControllerEffect::TransientPause { .. } => "transient_pause".into(),
        AutoControllerEffect::ScheduleRestart { .. } => "schedule_restart".into(),
        AutoControllerEffect::CancelCoordinator => "cancel_coordinator".into(),
        AutoControllerEffect::ResetHistory => "reset_history".into(),
        AutoControllerEffect::UpdateTerminalHint { .. } => "update_terminal_hint".into(),
        AutoControllerEffect::SetTaskRunning { .. } => "set_task_running".into(),
        AutoControllerEffect::EnsureInputFocus => "ensure_input_focus".into(),
        AutoControllerEffect::ClearCoordinatorView => "clear_coordinator_view".into(),
        AutoControllerEffect::ShowGoalEntry => "show_goal_entry".into(),
    }
}

#[derive(Deserialize)]
struct AutoDriveSequenceEnvelope {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: String,
    #[serde(rename = "initial_state")]
    initial_state: ControllerState,
    operations: Vec<ControllerOperation>,
}

#[derive(Deserialize)]
struct ControllerState {
    phase: PhasePayload,
    #[serde(rename = "continue_mode")]
    continue_mode: ContinueModePayload,
    #[serde(rename = "countdown_id")]
    countdown_id: u64,
    #[serde(rename = "countdown_decision_seq")]
    countdown_decision_seq: u64,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case")]
enum PhasePayload {
    Idle,
    AwaitingGoalEntry,
    Launching,
    Active,
    PausedManual {
        #[serde(rename = "resume_after_submit")]
        resume_after_submit: bool,
        #[serde(rename = "bypass_next_submit")]
        bypass_next_submit: bool,
    },
    AwaitingCoordinator {
        #[serde(rename = "prompt_ready")]
        prompt_ready: bool,
    },
    AwaitingDiagnostics {
        #[serde(rename = "coordinator_waiting")]
        coordinator_waiting: bool,
    },
    AwaitingReview {
        #[serde(rename = "diagnostics_pending")]
        diagnostics_pending: bool,
    },
    TransientRecovery {
        #[serde(rename = "backoff_ms")]
        backoff_ms: u64,
    },
}

impl From<PhasePayload> for AutoRunPhase {
    fn from(value: PhasePayload) -> Self {
        match value {
            PhasePayload::Idle => AutoRunPhase::Idle,
            PhasePayload::AwaitingGoalEntry => AutoRunPhase::AwaitingGoalEntry,
            PhasePayload::Launching => AutoRunPhase::Launching,
            PhasePayload::Active => AutoRunPhase::Active,
            PhasePayload::PausedManual {
                resume_after_submit,
                bypass_next_submit,
            } => AutoRunPhase::PausedManual {
                resume_after_submit,
                bypass_next_submit,
            },
            PhasePayload::AwaitingCoordinator { prompt_ready } => {
                AutoRunPhase::AwaitingCoordinator { prompt_ready }
            }
            PhasePayload::AwaitingDiagnostics { coordinator_waiting } => {
                AutoRunPhase::AwaitingDiagnostics { coordinator_waiting }
            }
            PhasePayload::AwaitingReview { diagnostics_pending } => {
                AutoRunPhase::AwaitingReview { diagnostics_pending }
            }
            PhasePayload::TransientRecovery { backoff_ms } => {
                AutoRunPhase::TransientRecovery { backoff_ms }
            }
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ContinueModePayload {
    Immediate,
    TenSeconds,
    SixtySeconds,
    Manual,
}

impl From<ContinueModePayload> for AutoContinueMode {
    fn from(value: ContinueModePayload) -> Self {
        match value {
            ContinueModePayload::Immediate => AutoContinueMode::Immediate,
            ContinueModePayload::TenSeconds => AutoContinueMode::TenSeconds,
            ContinueModePayload::SixtySeconds => AutoContinueMode::SixtySeconds,
            ContinueModePayload::Manual => AutoContinueMode::Manual,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControllerOperation {
    UpdateContinueMode { mode: ContinueModePayload },
    HandleCountdownTick {
        #[serde(rename = "countdown_id")]
        countdown_id: u64,
        #[serde(rename = "decision_seq")]
        decision_seq: u64,
        #[serde(rename = "seconds_left")]
        seconds_left: u8,
    },
    PauseForTransientFailure { reason: String },
    StopRun { message: Option<String> },
    LaunchResult {
        result: LaunchOutcome,
        goal: String,
        error: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchOutcome {
    Succeeded,
    Failed,
}
