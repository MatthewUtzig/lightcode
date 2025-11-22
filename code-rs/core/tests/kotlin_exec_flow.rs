use code_core::config::{find_code_home, Config, ConfigOverrides, ConfigToml, EngineMode};
use code_core::protocol::{AskForApproval, Event, EventMsg, InputItem, Op, SandboxPolicy};
use code_core::{AuthManager, ConversationManager, KotlinCoreHost};
use code_protocol::protocol::SessionSource;
use std::env;
use std::path::{Path, PathBuf};
use std::thread;
use tokio::time::{timeout, Duration};
use serde_json::json;

const EVENT_TIMEOUT_SECS: u64 = 60;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kotlin_exec_and_patch_flow_runs_through_pipeline() {
    let workspace = env::current_dir().expect("current dir");
    let code_home = find_code_home().expect("code home");

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/simple_model_fixture.json");
    set_kotlin_fixture_env(&fixture_path);
    unsafe {
        env::set_var("CODEX_EXPERIMENTAL_KOTLIN_COORDINATOR", "1");
    }

    let mut config = Config::load_from_base_config_with_overrides(
        ConfigToml::default(),
        ConfigOverrides::default(),
        code_home.clone(),
    )
    .expect("load config");
    config.cwd = workspace.clone();
    config.engine_mode = EngineMode::Kotlin;
    config.approval_policy = AskForApproval::UnlessTrusted;
    config.sandbox_policy = SandboxPolicy::DangerFullAccess;

    let auth_manager = AuthManager::shared(code_home);
    let manager = ConversationManager::new(auth_manager, SessionSource::Cli);
    let conversation = manager
        .new_conversation(config)
        .await
        .expect("spawn conversation");
    let codex = conversation.conversation;

    codex
        .submit(Op::UserInput {
            items: vec![InputItem::Text {
                text: "trigger kotlin exec".into(),
            }],
        })
        .await
        .expect("submit user input");

    let exec_message =
        wait_for_agent_message_matching(&codex, |text| text.contains("Kotlin coordinator pending exec"))
            .await;
    assert!(exec_message.contains("ls -la"), "expected exec fence to surface command");

    let patch_message =
        wait_for_agent_message_matching(&codex, |text| text.contains("Kotlin coordinator pending patch"))
            .await;
    assert!(patch_message.contains("*** Begin Patch"), "expected patch fence to surface diff");

    let _ = codex.submit(Op::Shutdown).await;
}

#[test]
fn kotlin_control_stop_emits_stop_ack_event() {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/simple_model_fixture.json");
    set_kotlin_fixture_env(&fixture_path);

    let mut host = KotlinCoreHost::new().expect("construct Kotlin host");
    let payload = json!({
        "type": "control",
        "command": "stop",
    });
    host.submit_json(&payload).expect("submit stop control");

    for _ in 0..5 {
        let events = host.poll_events().expect("poll events");
        if events.iter().any(|event| {
            if event.kind != "kotlin_coordinator_event" {
                return false;
            }
            let Some(payload) = event.payload.get("decisions").and_then(|d| d.as_array()) else {
                return false;
            };
            payload.iter().any(|decision| {
                decision
                    .get("type")
                    .and_then(|value| value.as_str())
                    .map_or(false, |value| value == "stop_ack")
            })
        }) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }

    panic!("expected stop_ack decision from Kotlin host");
}

async fn wait_for_event<T>(
    codex: &code_core::CodexConversation,
    mut matcher: impl FnMut(&Event) -> Option<T>,
) -> T {
    loop {
        let event = timeout(Duration::from_secs(EVENT_TIMEOUT_SECS), codex.next_event())
            .await
            .expect("timeout waiting for event")
            .expect("event stream closed");
        if let EventMsg::Error(err) = &event.msg {
            if err.message.contains("Authentication required") {
                panic!("Authentication required for Kotlin integration tests. Run `code login`." );
            }
            panic!("codex error event: {}", err.message);
        }
        if let Some(value) = matcher(&event) {
            return value;
        }
    }
}

async fn wait_for_agent_message(codex: &code_core::CodexConversation) -> String {
    wait_for_event(codex, |event| match &event.msg {
        EventMsg::AgentMessage(ev) => Some(ev.message.clone()),
        _ => None,
    })
    .await
}

async fn wait_for_agent_message_matching(
    codex: &code_core::CodexConversation,
    mut predicate: impl FnMut(&str) -> bool,
) -> String {
    loop {
        let text = wait_for_agent_message(codex).await;
        if predicate(&text) {
            return text;
        }
    }
}

fn set_kotlin_fixture_env(path: &Path) {
    unsafe {
        env::set_var("CODE_KOTLIN_SIMPLE_MODEL_FIXTURE", path);
    }
}
