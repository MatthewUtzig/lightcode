use anyhow::{anyhow, Result};
use jni::objects::{JObject, JString, JValue};
use jni::{InitArgsBuilder, JavaVM};
use once_cell::sync::OnceCell;

mod classpath;

use crate::classpath::resolve_classpath;

static JVM: OnceCell<JavaVM> = OnceCell::new();

fn java_vm() -> Result<&'static JavaVM> {
    JVM.get_or_try_init(|| {
        let classpath = resolve_classpath()?;
        let option = format!("-Djava.class.path={classpath}");
        let args = InitArgsBuilder::new()
            .option(&option)
            .build()
            .map_err(|err| anyhow!("failed to build JVM args: {err}"))?;
        JavaVM::new(args).map_err(|err| anyhow!("failed to create JVM: {err}"))
    })
}

fn call_static_str(method: &str, signature: &str, args: &[JValue<'_, '_>]) -> Result<String> {
    let vm = java_vm()?;
    let mut env = vm.attach_current_thread().map_err(|err| anyhow!("attach thread failed: {err}"))?;
    let class = env
        .find_class("ai/lightcode/core/engine/CoreEngineHost")
        .map_err(|err| anyhow!("failed to find CoreEngineHost: {err}"))?;
    let result = env
        .call_static_method(class, method, signature, args)
        .map_err(|err| anyhow!("call {method} failed: {err}"))?;
    let obj = result.l().map_err(|err| anyhow!("{method} returned non-object: {err}"))?;
    let jstr: JString = JString::from(obj);
    let rust_str: String = env
        .get_string(&jstr)
        .map_err(|err| anyhow!("failed to read JVM string: {err}"))?
        .into();
    Ok(rust_str)
}

pub fn start_session(config_json: &str) -> Result<String> {
    let vm = java_vm()?;
    let env = vm.attach_current_thread().map_err(|err| anyhow!("attach thread failed: {err}"))?;
    let arg = env
        .new_string(config_json)
        .map_err(|err| anyhow!("failed to create string: {err}"))?;
    let arg_obj = JObject::from(arg);
    call_static_str(
        "startSession",
        "(Ljava/lang/String;)Ljava/lang/String;",
        &[JValue::Object(&arg_obj)],
    )
}

pub fn submit_turn(session_id: &str, submission_json: &str) -> Result<String> {
    let vm = java_vm()?;
    let env = vm.attach_current_thread().map_err(|err| anyhow!("attach thread failed: {err}"))?;
    let sid = env
        .new_string(session_id)
        .map_err(|err| anyhow!("failed to create string: {err}"))?;
    let payload = env
        .new_string(submission_json)
        .map_err(|err| anyhow!("failed to create string: {err}"))?;
    let sid_obj = JObject::from(sid);
    let payload_obj = JObject::from(payload);
    call_static_str(
        "submitTurn",
        "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
        &[
            JValue::Object(&sid_obj),
            JValue::Object(&payload_obj),
        ],
    )
}

pub fn poll_events(session_id: &str, cursor_json: &str) -> Result<String> {
    let vm = java_vm()?;
    let env = vm.attach_current_thread().map_err(|err| anyhow!("attach thread failed: {err}"))?;
    let sid = env
        .new_string(session_id)
        .map_err(|err| anyhow!("failed to create string: {err}"))?;
    let cursor = env
        .new_string(cursor_json)
        .map_err(|err| anyhow!("failed to create string: {err}"))?;
    let sid_obj = JObject::from(sid);
    let cursor_obj = JObject::from(cursor);
    let raw = call_static_str(
        "pollEvents",
        "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
        &[
            JValue::Object(&sid_obj),
            JValue::Object(&cursor_obj),
        ],
    )?;
    Ok(raw)
}

pub fn close_session(session_id: &str) -> Result<()> {
    let vm = java_vm()?;
    let env = vm.attach_current_thread().map_err(|err| anyhow!("attach thread failed: {err}"))?;
    let sid = env
        .new_string(session_id)
        .map_err(|err| anyhow!("failed to create string: {err}"))?;
    let sid_obj = JObject::from(sid);
    let _ = call_static_str(
        "closeSession",
        "(Ljava/lang/String;)Ljava/lang/String;",
        &[JValue::Object(&sid_obj)],
    )?;
    Ok(())
}

pub fn run_auto_drive_sequence_raw(submission_json: &str) -> Result<String> {
    let vm = java_vm()?;
    let env = vm.attach_current_thread().map_err(|err| anyhow!("attach thread failed: {err}"))?;
    let payload = env
        .new_string(submission_json)
        .map_err(|err| anyhow!("failed to create string: {err}"))?;
    let payload_obj = JObject::from(payload);
    call_static_str(
        "runAutoDriveSequenceRaw",
        "(Ljava/lang/String;)Ljava/lang/String;",
        &[JValue::Object(&payload_obj)],
    )
}
